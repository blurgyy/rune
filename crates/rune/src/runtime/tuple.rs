use crate::runtime::{
    ConstValue, FromValue, Mut, Ref, ToValue, Value, Vm, VmError, VmErrorKind, TUPLE_TYPE,
};
use std::fmt;
use std::ops;
use std::slice;

/// Struct representing a dynamic anonymous object.
#[derive(Clone)]
#[repr(transparent)]
pub struct Tuple {
    inner: Box<[Value]>,
}

impl Tuple {
    fn empty() -> Self {
        Self {
            inner: Vec::new().into_boxed_slice(),
        }
    }

    /// Convert into inner std boxed slice.
    pub fn into_inner(self) -> Box<[Value]> {
        self.inner
    }

    /// Returns `true` if the dynamic tuple contains no elements.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of elements in the dynamic tuple, also referred to
    /// as its 'length'.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Get the value at the given index.
    pub fn get(&self, index: usize) -> Option<&Value> {
        self.inner.get(index)
    }

    /// Get the given value at the given index.
    pub fn get_value<T>(&self, index: usize) -> Result<Option<T>, VmError>
    where
        T: FromValue,
    {
        let value = match self.inner.get(index) {
            Some(value) => value.clone(),
            None => return Ok(None),
        };

        Ok(Some(T::from_value(value)?))
    }

    /// Get the mutable value at the given index.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut Value> {
        self.inner.get_mut(index)
    }

    /// Value pointer equals implementation for a Tuple.
    pub(crate) fn value_ptr_eq(vm: &mut Vm, a: &Self, b: &Self) -> Result<bool, VmError> {
        if a.len() != b.len() {
            return Ok(false);
        }

        for (a, b) in a.iter().zip(b.iter()) {
            if !Value::value_ptr_eq(vm, a, b)? {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

impl fmt::Debug for Tuple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(")?;

        let mut it = self.inner.iter();
        let last = it.next_back();

        for el in it {
            write!(f, "{:?}, ", el)?;
        }

        if let Some(last) = last {
            write!(f, "{:?}", last)?;
        }

        write!(f, ")")?;
        Ok(())
    }
}

impl ops::Deref for Tuple {
    type Target = [Value];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl ops::DerefMut for Tuple {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<'a> IntoIterator for &'a Tuple {
    type Item = &'a Value;
    type IntoIter = slice::Iter<'a, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter()
    }
}

impl<'a> IntoIterator for &'a mut Tuple {
    type Item = &'a mut Value;
    type IntoIter = slice::IterMut<'a, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter_mut()
    }
}

impl From<Vec<Value>> for Tuple {
    fn from(vec: Vec<Value>) -> Self {
        Self {
            inner: vec.into_boxed_slice(),
        }
    }
}

impl From<Box<[Value]>> for Tuple {
    fn from(inner: Box<[Value]>) -> Self {
        Self { inner }
    }
}

impl From<Box<[ConstValue]>> for Tuple {
    fn from(inner: Box<[ConstValue]>) -> Self {
        let mut out = Vec::with_capacity(inner.len());

        for value in inner.into_vec() {
            out.push(value.into_value());
        }

        Self {
            inner: out.into_boxed_slice(),
        }
    }
}

impl FromValue for Mut<Tuple> {
    fn from_value(value: Value) -> Result<Self, VmError> {
        Ok(value.into_tuple()?.into_mut()?)
    }
}

impl FromValue for Ref<Tuple> {
    fn from_value(value: Value) -> Result<Self, VmError> {
        Ok(value.into_tuple()?.into_ref()?)
    }
}

impl FromValue for Tuple {
    fn from_value(value: Value) -> Result<Self, VmError> {
        match value {
            Value::Unit => Ok(Self::empty()),
            Value::Tuple(tuple) => Ok(tuple.take()?),
            actual => Err(VmError::expected::<Self>(actual.type_info()?)),
        }
    }
}

macro_rules! impl_tuple {
    () => ();

    ({$ty:ident, $var:ident, $count:expr}, $({$l_ty:ident, $l_var:ident, $l_count:expr},)*) => {
        impl_tuple!{@impl $count, {$ty, $var, $count}, $({$l_ty, $l_var, $l_count},)*}
        impl_tuple!{$({$l_ty, $l_var, $l_count},)*}
    };

    (@impl $count:expr, $({$ty:ident, $var:ident, $ignore_count:expr},)*) => {
        impl_static_type!(impl <$($ty),*> ($($ty,)*) => TUPLE_TYPE);

        impl <$($ty,)*> FromValue for ($($ty,)*)
        where
            $($ty: FromValue,)*
        {
            fn from_value(value: Value) -> Result<Self, VmError> {
                let tuple = value.into_tuple()?.take()?;

                if tuple.len() != $count {
                    return Err(VmError::from(VmErrorKind::ExpectedTupleLength {
                        actual: tuple.len(),
                        expected: $count,
                    }));
                }

                #[allow(unused_mut, unused_variables)]
                let mut it = Vec::from(tuple.into_inner()).into_iter();

                $(
                    let $var = match it.next() {
                        Some(value) => <$ty>::from_value(value)?,
                        None => {
                            return Err(VmError::from(VmErrorKind::IterationError));
                        },
                    };
                )*

                Ok(($($var,)*))
            }
        }

        impl <$($ty,)*> ToValue for ($($ty,)*)
        where
            $($ty: ToValue,)*
        {
            fn to_value(self) -> Result<Value, VmError> {
                let ($($var,)*) = self;
                $(let $var = $var.to_value()?;)*
                Ok(Value::from(Tuple::from(vec![$($var,)*])))
            }
        }
    };
}

repeat_macro!(impl_tuple);
