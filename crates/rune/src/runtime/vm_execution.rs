use crate::runtime::budget;
use crate::runtime::{
    Generator, GeneratorState, Stream, Value, Vm, VmError, VmErrorKind, VmHalt, VmHaltInfo,
};
use crate::shared::AssertSend;
use std::fmt;
use std::future::Future;
use std::mem::take;

/// The state of an execution. We keep track of this because it's important to
/// correctly interact with functions that yield (like generators and streams)
/// by initially just calling the function, then by providing a value pushed
/// onto the stack.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum ExecutionState {
    /// The initial state of an execution.
    Initial,
    /// The resumed state of an execution. This expects a value to be pushed
    /// onto the virtual machine before it is continued.
    Resumed,
}

impl fmt::Display for ExecutionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutionState::Initial => write!(f, "initial"),
            ExecutionState::Resumed => write!(f, "resumed"),
        }
    }
}

/// The execution environment for a virtual machine.
///
/// When an execution is dropped, the stack of the stack of the head machine
/// will be cleared.
pub struct VmExecution<T = Vm>
where
    T: AsMut<Vm>,
{
    /// The current head vm which holds the execution.
    head: T,
    /// The state of an execution.
    state: ExecutionState,
    /// The current stack of virtual machines and the execution state that must
    /// be restored once one is popped.
    vms: Vec<(Vm, ExecutionState)>,
}

macro_rules! vm {
    ($slf:expr) => {
        match $slf.vms.last() {
            Some((vm, _)) => vm,
            None => $slf.head.as_ref(),
        }
    };
}

macro_rules! vm_mut {
    ($slf:expr) => {
        match $slf.vms.last_mut() {
            Some((vm, _)) => vm,
            None => $slf.head.as_mut(),
        }
    };
}

impl<T> VmExecution<T>
where
    T: AsMut<Vm>,
{
    /// Construct an execution from a virtual machine.
    pub(crate) fn new(head: T) -> Self {
        Self {
            head,
            vms: vec![],
            state: ExecutionState::Initial,
        }
    }

    /// Test if the current execution state is resumed.
    pub(crate) fn is_resumed(&self) -> bool {
        matches!(self.state, ExecutionState::Resumed)
    }

    /// Coerce the current execution into a generator if appropriate.
    ///
    /// ```
    /// use rune::{Context, FromValue, Vm};
    /// use std::sync::Arc;
    ///
    /// # fn main() -> rune::Result<()> {
    /// let mut sources = rune::sources! {
    ///     entry => {
    ///         pub fn main() {
    ///             yield 1;
    ///             yield 2;
    ///         }
    ///     }
    /// };
    ///
    /// let unit = rune::prepare(&mut sources).build()?;
    ///
    /// let mut vm = Vm::without_runtime(Arc::new(unit));
    /// let mut generator = vm.execute(["main"], ())?.into_generator()?;
    ///
    /// let mut n = 1i64;
    ///
    /// while let Some(value) = generator.next()? {
    ///     let value = i64::from_value(value)?;
    ///     assert_eq!(value, n);
    ///     n += 1;
    /// }
    /// # Ok(()) }
    /// ```
    pub fn into_generator(self) -> Result<Generator<T>, VmError> {
        Ok(Generator::from_execution(self))
    }

    /// Coerce the current execution into a stream if appropriate.
    ///
    /// ```
    /// use rune::{Context, FromValue, Vm};
    /// use std::sync::Arc;
    ///
    /// # #[tokio::main] async fn main() -> rune::Result<()> {
    /// let mut sources = rune::sources! {
    ///     entry => {
    ///         pub async fn main() {
    ///             yield 1;
    ///             yield 2;
    ///         }
    ///     }
    /// };
    ///
    /// let unit = rune::prepare(&mut sources).build()?;
    ///
    /// let mut vm = Vm::without_runtime(Arc::new(unit));
    /// let mut stream = vm.execute(["main"], ())?.into_stream()?;
    ///
    /// let mut n = 1i64;
    ///
    /// while let Some(value) = stream.next().await? {
    ///     let value = i64::from_value(value)?;
    ///     assert_eq!(value, n);
    ///     n += 1;
    /// }
    /// # Ok(()) }
    /// ```
    pub fn into_stream(self) -> Result<Stream<T>, VmError> {
        Ok(Stream::from_execution(self))
    }

    /// Get a reference to the current virtual machine.
    pub fn vm(&self) -> &Vm
    where
        T: AsRef<Vm>,
    {
        vm!(self)
    }

    /// Get a mutable reference the current virtual machine.
    pub fn vm_mut(&mut self) -> &mut Vm {
        vm_mut!(self)
    }

    /// Complete the current execution without support for async instructions.
    ///
    /// This will error if the execution is suspended through yielding.
    pub async fn async_complete(&mut self) -> Result<Value, VmError> {
        match self.async_resume().await? {
            GeneratorState::Complete(value) => Ok(value),
            GeneratorState::Yielded(..) => Err(VmError::from(VmErrorKind::Halted {
                halt: VmHaltInfo::Yielded,
            })),
        }
    }

    /// Complete the current execution without support for async instructions.
    ///
    /// If any async instructions are encountered, this will error. This will
    /// also error if the execution is suspended through yielding.
    pub fn complete(&mut self) -> Result<Value, VmError> {
        match self.resume()? {
            GeneratorState::Complete(value) => Ok(value),
            GeneratorState::Yielded(..) => Err(VmError::from(VmErrorKind::Halted {
                halt: VmHaltInfo::Yielded,
            })),
        }
    }

    /// Resume the current execution with the given value and resume
    /// asynchronous execution.
    pub async fn async_resume_with(&mut self, value: Value) -> Result<GeneratorState, VmError> {
        if !matches!(self.state, ExecutionState::Resumed) {
            return Err(VmErrorKind::ExpectedExecutionState {
                expected: ExecutionState::Resumed,
                actual: self.state,
            }
            .into());
        }

        vm_mut!(self).stack_mut().push(value);
        self.inner_async_resume().await
    }

    /// Resume the current execution with support for async instructions.
    ///
    /// If the function being executed is a generator or stream this will resume
    /// it while returning a unit from the current `yield`.
    pub async fn async_resume(&mut self) -> Result<GeneratorState, VmError> {
        if matches!(self.state, ExecutionState::Resumed) {
            vm_mut!(self).stack_mut().push(Value::Unit);
        } else {
            self.state = ExecutionState::Resumed;
        }

        self.inner_async_resume().await
    }

    async fn inner_async_resume(&mut self) -> Result<GeneratorState, VmError> {
        loop {
            let len = self.vms.len();
            let vm = vm_mut!(self);

            match Self::run(vm)? {
                VmHalt::Exited => (),
                VmHalt::Awaited(awaited) => {
                    awaited.into_vm(vm).await?;
                    continue;
                }
                VmHalt::VmCall(vm_call) => {
                    vm_call.into_execution(self)?;
                    continue;
                }
                VmHalt::Yielded => {
                    let value = vm.stack_mut().pop()?;
                    return Ok(GeneratorState::Yielded(value));
                }
                halt => {
                    return Err(VmError::from(VmErrorKind::Halted {
                        halt: halt.into_info(),
                    }))
                }
            }

            if len == 0 {
                let value = self.end()?;
                return Ok(GeneratorState::Complete(value));
            }

            self.pop_vm()?;
        }
    }

    /// Resume the current execution with the given value and resume synchronous
    /// execution.
    pub fn resume_with(&mut self, value: Value) -> Result<GeneratorState, VmError> {
        if !matches!(self.state, ExecutionState::Resumed) {
            return Err(VmErrorKind::ExpectedExecutionState {
                expected: ExecutionState::Resumed,
                actual: self.state,
            }
            .into());
        }

        vm_mut!(self).stack_mut().push(value);
        self.inner_resume()
    }

    /// Resume the current execution without support for async instructions.
    ///
    /// If the function being executed is a generator or stream this will resume
    /// it while returning a unit from the current `yield`.
    ///
    /// If any async instructions are encountered, this will error with
    /// [VmErrorKind::Halted].
    pub fn resume(&mut self) -> Result<GeneratorState, VmError> {
        if matches!(self.state, ExecutionState::Resumed) {
            vm_mut!(self).stack_mut().push(Value::Unit);
        } else {
            self.state = ExecutionState::Resumed;
        }

        self.inner_resume()
    }

    fn inner_resume(&mut self) -> Result<GeneratorState, VmError> {
        loop {
            let len = self.vms.len();
            let vm = vm_mut!(self);

            match Self::run(vm)? {
                VmHalt::Exited => (),
                VmHalt::VmCall(vm_call) => {
                    vm_call.into_execution(self)?;
                    continue;
                }
                VmHalt::Yielded => {
                    let value = vm.stack_mut().pop()?;
                    return Ok(GeneratorState::Yielded(value));
                }
                halt => {
                    return Err(VmError::from(VmErrorKind::Halted {
                        halt: halt.into_info(),
                    }))
                }
            }

            if len == 0 {
                let value = self.end()?;
                return Ok(GeneratorState::Complete(value));
            }

            self.pop_vm()?;
        }
    }

    /// Step the single execution for one step without support for async
    /// instructions.
    ///
    /// If any async instructions are encountered, this will error.
    pub fn step(&mut self) -> Result<Option<Value>, VmError> {
        let len = self.vms.len();
        let vm = vm_mut!(self);

        match budget::with(1, || Self::run(vm)).call()? {
            VmHalt::Exited => (),
            VmHalt::VmCall(vm_call) => {
                vm_call.into_execution(self)?;
                return Ok(None);
            }
            VmHalt::Limited => return Ok(None),
            halt => {
                return Err(VmError::from(VmErrorKind::Halted {
                    halt: halt.into_info(),
                }))
            }
        }

        if len == 0 {
            let value = self.end()?;
            return Ok(Some(value));
        }

        self.pop_vm()?;
        Ok(None)
    }

    /// Step the single execution for one step with support for async
    /// instructions.
    pub async fn async_step(&mut self) -> Result<Option<Value>, VmError> {
        let len = self.vms.len();
        let vm = vm_mut!(self);

        match budget::with(1, || Self::run(vm)).call()? {
            VmHalt::Exited => (),
            VmHalt::Awaited(awaited) => {
                awaited.into_vm(vm).await?;
                return Ok(None);
            }
            VmHalt::VmCall(vm_call) => {
                vm_call.into_execution(self)?;
                return Ok(None);
            }
            VmHalt::Limited => return Ok(None),
            halt => {
                return Err(VmError::from(VmErrorKind::Halted {
                    halt: halt.into_info(),
                }))
            }
        }

        if len == 0 {
            let value = self.end()?;
            return Ok(Some(value));
        }

        self.pop_vm()?;
        Ok(None)
    }

    /// End execution and perform debug checks.
    pub(crate) fn end(&mut self) -> Result<Value, VmError> {
        let vm = self.head.as_mut();
        let value = vm.stack_mut().pop()?;
        debug_assert!(self.vms.is_empty(), "execution vms should be empty");
        Ok(value)
    }

    /// Push a virtual machine state onto the execution.
    pub(crate) fn push_vm(&mut self, vm: Vm) {
        self.vms.push((vm, self.state));
        self.state = ExecutionState::Initial;
    }

    /// Pop a virtual machine state from the execution and transfer the top of
    /// the stack from the popped machine.
    fn pop_vm(&mut self) -> Result<(), VmError> {
        let (mut from, state) = self.vms.pop().ok_or(VmErrorKind::NoRunningVm)?;

        let stack = from.stack_mut();
        let value = stack.pop()?;
        debug_assert!(stack.is_empty(), "vm stack not clean");

        let onto = vm_mut!(self);
        onto.stack_mut().push(value);
        onto.advance();
        self.state = state;
        Ok(())
    }

    #[inline]
    fn run(vm: &mut Vm) -> Result<VmHalt, VmError> {
        match vm.run() {
            Ok(reason) => Ok(reason),
            Err(error) => Err(error.into_unwinded(vm.unit(), vm.ip(), vm.call_frames().to_vec())),
        }
    }
}

impl VmExecution<&mut Vm> {
    /// Convert the current execution into one which owns its virtual machine.
    pub fn into_owned(self) -> VmExecution<Vm> {
        let stack = take(self.head.stack_mut());
        let head = Vm::with_stack(self.head.context().clone(), self.head.unit().clone(), stack);

        VmExecution {
            head,
            vms: self.vms,
            state: self.state,
        }
    }
}

/// A wrapper that makes [`VmExecution`] [`Send`].
///
/// This is accomplished by preventing any [`Value`] from escaping the [`Vm`].
/// As long as this is maintained, it is safe to send the execution across,
/// threads, and therefore schedule the future associated with the execution on
/// a thread pool like Tokio's through [tokio::spawn].
///
/// [tokio::spawn]: https://docs.rs/tokio/0/tokio/runtime/struct.Runtime.html#method.spawn
pub struct VmSendExecution(pub(crate) VmExecution<Vm>);

// Safety: we wrap all APIs around the [VmExecution], preventing values from
// escaping from contained virtual machine.
unsafe impl Send for VmSendExecution {}

impl VmSendExecution {
    /// Complete the current execution with support for async instructions.
    ///
    /// This requires that the result of the Vm is converted into a
    /// [crate::FromValue] that also implements [Send],  which prevents non-Send
    /// values from escaping from the virtual machine.
    pub fn async_complete(
        mut self,
    ) -> impl Future<Output = Result<Value, VmError>> + Send + 'static {
        let future = async move {
            let result = self.0.async_resume().await?;

            match result {
                GeneratorState::Complete(value) => Ok(value),
                GeneratorState::Yielded(..) => Err(VmError::from(VmErrorKind::Halted {
                    halt: VmHaltInfo::Yielded,
                })),
            }
        };

        // Safety: we wrap all APIs around the [VmExecution], preventing values
        // from escaping from contained virtual machine.
        unsafe { AssertSend::new(future) }
    }
}
