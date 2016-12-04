// Copyright 2016 Jason Lingle
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::borrow::{Borrow, ToOwned};
use std::convert::{AsRef, From};
use std::cmp;
use std::fmt;
use std::mem;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, DerefMut};
use std::slice;

/// Miscelaneous things used to integrate other code with Supercow, but which
/// are not of interest to end users.
pub mod aux {
    use std::borrow::Borrow;
    use std::ffi::{CStr, OsStr};
    use std::path::Path;
    use std::rc::Rc;
    use std::slice;
    use std::sync::Arc;

    /// Marker trait indicating a `Deref`-like which always returns the same
    /// reference.
    ///
    /// This is not indended for general use outside Supercow. Notably, `Box`
    /// and mundane references satisfy this trait's requirements, but
    /// deliberately do not implement it. It is also not a subtrait of `Deref`
    /// due to some additional special logic around boxes.
    ///
    /// ## Unsafety
    ///
    /// Behaviour is undefined if the implementation does not always return the
    /// same reference from `deref()` for any particular implementing value
    /// (including if that value is moved).
    pub unsafe trait ConstDeref {
        type Target : ?Sized;
        fn const_deref(&self) -> &Self::Target;
    }

    unsafe impl<T : ?Sized> ConstDeref for Rc<T> {
        type Target = T;
        fn const_deref(&self) -> &T { self }
    }

    unsafe impl<T : ?Sized> ConstDeref for Arc<T> {
        type Target = T;
        fn const_deref(&self) -> &T { self }
    }

    unsafe impl<T : ConstDeref + ?Sized> ConstDeref for Box<T> {
        type Target = T::Target;
        fn const_deref(&self) -> &T::Target {
            (**self).const_deref()
        }
    }

    /// Extension of `Borrow` used to allow `Supercow::to_mut()` to work
    /// safely.
    pub unsafe trait SafeBorrow<T : ?Sized>: Borrow<T> {
        /// Given `ptr`, which was obtained from a prior call to
        /// `Self::borrow()`, return a value with the same nominal lifetime
        /// which is guaranteed to survive mutations to `Self`.
        ///
        /// Types which implement `Borrow` by pure, constant pointer arithmetic
        /// on `self` can simply return `ptr` unmodified. Other types typically
        /// need to provide some static reference, such as the empty string for
        /// `&str`.
        ///
        /// ## Unsafety
        ///
        /// Behaviour is undefined if this call returns `ptr`, but a mutation
        /// to `Self` could invalidate the reference.
        fn borrow_replacement<'a>(ptr: &'a T) -> &'a T;
    }
    unsafe impl<T : ?Sized> SafeBorrow<T> for T {
        fn borrow_replacement(ptr: &T) -> &T { ptr }
    }
    unsafe impl<B, T> SafeBorrow<[B]> for T where T : Borrow<[B]> {
        fn borrow_replacement(_: &[B]) -> &[B] {
            unsafe {
                slice::from_raw_parts(1 as usize as *const B, 0)
            }
        }
    }
    unsafe impl<T> SafeBorrow<str> for T where T : Borrow<str> {
        fn borrow_replacement(_: &str) -> &str { "" }
    }
    unsafe impl<T> SafeBorrow<CStr> for T
    where T : Borrow<CStr> {
        fn borrow_replacement(_: &CStr) -> &CStr {
            static EMPTY_CSTR: &'static [u8] = &[0];
            unsafe {
                CStr::from_bytes_with_nul_unchecked(EMPTY_CSTR)
            }
        }
    }
    unsafe impl<T> SafeBorrow<OsStr> for T
    where T : Borrow<OsStr> {
        fn borrow_replacement(_: &OsStr) -> &OsStr {
            OsStr::new("")
        }
    }
    unsafe impl<T> SafeBorrow<Path> for T
    where T : Borrow<Path> {
        fn borrow_replacement(_: &Path) -> &Path {
            Path::new("")
        }
    }

    /// Marker trait identifying a reference type which begins with an absolute
    /// address and contains no other address-dependent information.
    ///
    /// `Supercow` expects to be able to read the first pointer-sized value of
    /// such a reference and perform address arithmetic upon it.
    ///
    /// There is no utility of applying this trait to anything other than a
    /// const reference.
    ///
    /// ## Unsafety
    ///
    /// Behaviour is undefined if a marked type does not begin with a real
    /// pointer to a value (with the usual exception of ZSTs) or if other parts
    /// of the type contain address-dependent information.
    ///
    /// Behaviour is undefined if the reference has any `Drop` implementation,
    /// should a future Rust version make such things possible.
    pub unsafe trait PointerFirstRef { }
    unsafe impl<'a, T : Sized> PointerFirstRef for &'a T { }
    unsafe impl<'a, T> PointerFirstRef for &'a [T] { }
    unsafe impl<'a> PointerFirstRef for &'a str { }
    unsafe impl<'a> PointerFirstRef for &'a ::std::ffi::CStr { }
    unsafe impl<'a> PointerFirstRef for &'a ::std::ffi::OsStr { }
    unsafe impl<'a> PointerFirstRef for &'a ::std::path::Path { }

    /// Like `std::convert::From`, but without the blanket implementations that
    /// cause problems for `supercow_features!`.
    pub trait SharedFrom<T> {
        fn shared_from(t: T) -> Self;
    }
}

use self::aux::*;

/// Defines a "feature set" for a custom `Supercow` type.
///
/// ## Syntax
///
/// ```
/// #[macro_use] extern crate supercow;
///
/// # pub trait SomeTrait { }
/// # pub trait AnotherTrait { }
///
/// supercow_features!(
///   /// Some documentation, etc, if desired.
///   pub trait FeatureName: SomeTrait, AnotherTrait);
/// supercow_features!(
///   pub trait FeatureName2: Clone, SomeTrait, AnotherTrait);
///
/// # fn main() { }
/// ```
///
/// ## Semantics
///
/// A public trait named `FeatureName` is defined which extends all the listed
/// traits, other than `Clone`, and in addition to `ConstDeref`. If listed,
/// `Clone` *must* come first. If `Clone` is listed, the trait gains a
/// `clone_boxed()` method and `Box<FeatureName>` is `Clone`. All types which
/// implement all the listed traits (including `Clone`) and `ConstDeref`
/// implement `FeatureName`.
#[macro_export]
macro_rules! supercow_features {
    // It's unclear why $req:path doesn't work, but apparently constraints
    // allow neither `path` nor `ty`.
    ($(#[$meta:meta])* pub trait $feature_name:ident: Clone $(, $req:ident)*) => {
        $(#[$meta])*
        pub trait $feature_name<'a>: $($req +)* $crate::aux::ConstDeref + 'a {
            fn clone_boxed
                (&self)
                 -> Box<$feature_name<'a, Target = Self::Target> + 'a>;
        }
        impl<'a, T : 'a + $($req +)* Clone + $crate::aux::ConstDeref>
        $feature_name<'a> for T {
            fn clone_boxed
                (&self)
                 -> Box<$feature_name<'a, Target = Self::Target> + 'a>
            {
                let cloned: T = self.clone();
                Box::new(cloned)
            }
        }
        impl<'a, T : $feature_name<'a>> $crate::aux::SharedFrom<T>
        for Box<$feature_name<'a, Target = T::Target> + 'a> {
            fn shared_from(t: T) -> Self {
                Box::new(t)
            }
        }
        impl<'a, S : 'a + ?Sized> Clone for Box<$feature_name<'a, Target = S> + 'a> {
            fn clone(&self) -> Self {
                $feature_name::clone_boxed(&**self)
            }
        }
    };

    ($(#[$meta:meta])* pub trait $feature_name:ident: $($req:ident),*) => {
        $(#[$meta])*
        pub trait $feature_name<'a>: $($req +)* $crate::aux::ConstDeref + 'a {
        }
        impl<'a, T : 'a + $($req +)* $crate::aux::ConstDeref>
        $feature_name<'a> for T {
        }
        impl<'a, T : $feature_name<'a>> $crate::aux::SharedFrom<T>
        for Box<$feature_name<'a, Target = T::Target> + 'a> {
            fn shared_from(t: T) -> Self {
                Box::new(t)
            }
        }
    };
}

supercow_features!(
    /// The default feature set for shared `Supercow` references.
    pub trait DefaultFeatures: Clone);
supercow_features!(
    /// The feature set used for `ASupercow` references.
    pub trait SyncFeatures: Clone, Send, Sync);

pub struct Supercow<'a, OWNED, BORROWED : ?Sized = OWNED,
                    SHARED = Box<DefaultFeatures<'a, Target = BORROWED> + 'a>>
where BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    // In order to implement `Deref` in a branch-free fashion that isn't
    // sensitive to the Supercow being moved, we set `ptr_mask` and
    // `ptr_displacement` such that
    // `target = &*((&self & sext(ptr_mask)) + ptr_displacement)`
    // (arithmetic in terms of bytes, obviously).
    //
    // So for the three cases:
    //
    // Owned => ptr_mask = ~0u, ptr_displacement = offsetof(self, Owned.0)
    // Borrowed, Shared => ptr_mask = 0u, ptr_displacement = address
    //
    // In order to support DSTs, `ptr_displacement` is actually a reference to
    // `BORROWED`. We assume the first pointer-sized value is the actual
    // pointer (see `PointerFirstRef`). `ptr_displacement` may not actually be
    // dereferenced.
    ptr_displacement: &'a BORROWED,
    ptr_mask: usize,
    state: SupercowData<'a, OWNED, BORROWED, SHARED>,
}

enum SupercowData<'a, OWNED, BORROWED : 'a + ?Sized, SHARED> {
    Owned(OWNED),
    Borrowed(&'a BORROWED),
    Shared(SHARED),
}
use self::SupercowData::*;

impl<'a, OWNED, BORROWED : ?Sized, SHARED> Deref
for Supercow<'a, OWNED, BORROWED, SHARED>
where BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    type Target = BORROWED;
    #[inline]
    fn deref(&self) -> &BORROWED {
        let self_address = self as *const Self as usize;

        let mut target_ref = self.ptr_displacement;
        unsafe {
            let target_address: &mut usize = mem::transmute(&mut target_ref);
            let nominal_address = *target_address;
            *target_address = (self_address & self.ptr_mask) + nominal_address;
        }
        target_ref
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED>
Supercow<'a, OWNED, BORROWED, SHARED>
where OWNED : Borrow<BORROWED>,
      BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    pub fn owned(inner: OWNED) -> Self {
        Self::from_data(Owned(inner))
    }

    pub fn borrowed<T : Borrow<BORROWED> + ?Sized>(inner: &'a T) -> Self {
        Self::from_data(Borrowed(inner.borrow()))
    }

    pub fn shared<T>(inner: T) -> Self
    where SHARED : SharedFrom<T> {
        Self::from_data(Shared(SHARED::shared_from(inner)))
    }

    fn from_data(data: SupercowData<'a, OWNED, BORROWED, SHARED>) -> Self {
        let mut this = Supercow {
            ptr_mask: 0,
            ptr_displacement: unsafe { mem::uninitialized() },
            state: data,
        };
        this.set_ptr();
        this
    }

    fn set_ptr(&mut self) {
        {
            let borrowed_ptr = match self.state {
                Owned(ref r) => r.borrow(),
                Borrowed(r) => r,
                Shared(ref s) => s.const_deref(),
            };
            // There's no safe way to propagate `borrowed_ptr` into
            // `ptr_displacement` since the former has a borrow scoped to this
            // function.
            unsafe {
                let dst: &mut [u8] = slice::from_raw_parts_mut(
                    &mut self.ptr_displacement as *mut&'a BORROWED
                        as *mut u8,
                    mem::size_of::<&'a BORROWED>());
                let src: &[u8] = slice::from_raw_parts(
                    &borrowed_ptr as *const&BORROWED as *const u8,
                    mem::size_of::<&'a BORROWED>());
                dst.copy_from_slice(src);
            }
        }
        self.adjust_ptr();
    }

    fn adjust_ptr(&mut self) {
        // Use relative addressing if `ptr` is inside `self` and absolute
        // addressing otherwise.
        //
        // Ordinarily, `ptr` will always be inside `self` if the state is
        // `Owned`, and outside otherwise. However, it is possible to create
        // `Borrow` implementations that return arbitrary pointers, so we
        // handle the two cases like self instead.
        let self_start = self as *const Self as usize;
        let self_end = self_start + mem::size_of::<Self>();
        let addr: &mut usize = unsafe {
            mem::transmute(&mut self.ptr_displacement)
        };

        if *addr >= self_start && *addr < self_end {
            self.ptr_mask = !0;
            *addr -= self_start;
        } else {
            self.ptr_mask = 0;
        }
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED> From<OWNED>
for Supercow<'a, OWNED, BORROWED, SHARED>
where OWNED : Borrow<BORROWED>,
      BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    fn from(inner: OWNED) -> Self {
        Self::from_data(SupercowData::Owned(inner))
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED> From<&'a OWNED>
for Supercow<'a, OWNED, BORROWED, SHARED>
where OWNED : Borrow<BORROWED>,
      BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    fn from(inner: &'a OWNED) -> Self {
        Self::from_data(SupercowData::Borrowed(inner.borrow()))
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED>
Supercow<'a, OWNED, BORROWED, SHARED>
where OWNED : Borrow<BORROWED>,
      BORROWED : 'a + ToOwned<Owned = OWNED>,
      for<'l> &'l BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    pub fn take_ownership(this: Self)
                          -> Supercow<'static, OWNED, BORROWED, SHARED> {
        match this.state {
            Owned(o) => Supercow {
                ptr_mask: this.ptr_mask,
                ptr_displacement: unsafe {
                    &*(this.ptr_displacement as *const BORROWED)
                },
                state: Owned(o),
            },
            Borrowed(r) => Supercow::owned(r.to_owned()),
            Shared(ref s) => Supercow::owned(s.const_deref().to_owned()),
        }
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED>
Supercow<'a, OWNED, BORROWED, SHARED>
where OWNED : Borrow<BORROWED>,
      BORROWED : 'a + ToOwned<Owned = OWNED>,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    pub fn into_inner(this: Self) -> OWNED {
        match this.state {
            Owned(o) => o,
            Borrowed(r) => r.to_owned(),
            Shared(ref s) => s.const_deref().to_owned(),
        }
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED>
Supercow<'a, OWNED, BORROWED, SHARED>
where OWNED : SafeBorrow<BORROWED>,
      BORROWED : 'a + ToOwned<Owned = OWNED>,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    pub fn to_mut<'b>(&'b mut self) -> Ref<'a, 'b, OWNED, BORROWED, SHARED> {
        // Take ownership if we do not already have it
        let new = match self.state {
            Owned(_) => None,
            Borrowed(r) => Some(Self::owned(r.to_owned())),
            Shared(ref s) => Some(Self::owned(s.const_deref().to_owned())),
        };
        if let Some(new) = new {
            *self = new;
        }

        let r = match self.state {
            Owned(ref mut r) => r as *mut OWNED,
            _ => unreachable!(),
        };
        // Because mutating the owned value could invalidate the calculated
        // pointer we have, reset it to something that won't change, and then
        // recalculate it when the `Ref` is dropped.
        self.ptr_displacement =
            OWNED::borrow_replacement(self.ptr_displacement);
        self.adjust_ptr();

        Ref { r: r, parent: self }
    }
}

pub struct Ref<'a, 'b, OWNED, BORROWED : ?Sized, SHARED>
where 'a: 'b,
      OWNED : 'b + SafeBorrow<BORROWED>,
      BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : 'b + ConstDeref<Target = BORROWED> {
    r: *mut OWNED,
    parent: &'b mut Supercow<'a, OWNED, BORROWED, SHARED>,
}

impl<'a, 'b, OWNED, BORROWED : ?Sized, SHARED> Deref
for Ref<'a, 'b, OWNED, BORROWED, SHARED>
where 'a: 'b,
      OWNED : 'b + SafeBorrow<BORROWED>,
      BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : 'b + ConstDeref<Target = BORROWED> {
    type Target = OWNED;

    #[inline]
    fn deref(&self) -> &OWNED {
        unsafe { &*self.r }
    }
}

impl<'a, 'b, OWNED, BORROWED : ?Sized, SHARED> DerefMut
for Ref<'a, 'b, OWNED, BORROWED, SHARED>
where 'a: 'b,
      OWNED : 'b + SafeBorrow<BORROWED>,
      BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : 'b + ConstDeref<Target = BORROWED> {
    #[inline]
    fn deref_mut(&mut self) -> &mut OWNED {
        unsafe { &mut*self.r }
    }
}

impl<'a, 'b, OWNED, BORROWED : ?Sized, SHARED> Drop
for Ref<'a, 'b, OWNED, BORROWED, SHARED>
where 'a: 'b,
      OWNED : 'b + SafeBorrow<BORROWED>,
      BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : 'b + ConstDeref<Target = BORROWED> {
    #[inline]
    fn drop(&mut self) {
        // The value of `OWNED::borrow()` may have changed, so recompute
        // everything instead of backing the old values up.
        self.parent.set_ptr()
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED> Clone
for Supercow<'a, OWNED, BORROWED, SHARED>
where OWNED : Clone,
      BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : Clone + ConstDeref<Target = BORROWED> {
    fn clone(&self) -> Self {
        Supercow {
            ptr_mask: self.ptr_mask,
            ptr_displacement: self.ptr_displacement,
            state: match self.state {
                Owned(ref o) => Owned(o.clone()),
                Borrowed(r) => Borrowed(r),
                Shared(ref s) => Shared(s.clone()),
            }
        }
    }
}

macro_rules! deleg_fmt {
    ($tr:ident) => {
        impl<'a, OWNED, BORROWED : ?Sized, SHARED> fmt::$tr
        for Supercow<'a, OWNED, BORROWED, SHARED>
        where BORROWED : 'a + fmt::$tr,
              &'a BORROWED : PointerFirstRef,
              SHARED : ConstDeref<Target = BORROWED> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                (**self).fmt(f)
            }
        }
    }
}
deleg_fmt!(Binary);
deleg_fmt!(Debug);
deleg_fmt!(Display);
deleg_fmt!(LowerExp);
deleg_fmt!(LowerHex);
deleg_fmt!(Octal);
deleg_fmt!(Pointer);
deleg_fmt!(UpperExp);
deleg_fmt!(UpperHex);

impl<'a, OWNED, BORROWED : ?Sized, SHARED, T> cmp::PartialEq<T>
for Supercow<'a, OWNED, BORROWED, SHARED>
where T : Deref<Target = BORROWED>,
      BORROWED : 'a + PartialEq<BORROWED>,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    fn eq(&self, other: &T) -> bool {
        **self == **other
    }

    fn ne(&self, other: &T) -> bool {
        **self != **other
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED> cmp::Eq
for Supercow<'a, OWNED, BORROWED, SHARED>
where BORROWED : 'a + Eq,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> { }

impl<'a, OWNED, BORROWED : ?Sized, SHARED, T> cmp::PartialOrd<T>
for Supercow<'a, OWNED, BORROWED, SHARED>
where T : Deref<Target = BORROWED>,
      BORROWED : 'a + PartialOrd<BORROWED>,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    fn partial_cmp(&self, other: &T) -> Option<cmp::Ordering> {
        (**self).partial_cmp(other)
    }

    fn lt(&self, other: &T) -> bool {
        **self < **other
    }

    fn le(&self, other: &T) -> bool {
        **self <= **other
    }

    fn gt(&self, other: &T) -> bool {
        **self > **other
    }

    fn ge(&self, other: &T) -> bool {
        **self >= **other
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED> cmp::Ord
for Supercow<'a, OWNED, BORROWED, SHARED>
where BORROWED : 'a + cmp::Ord,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        (**self).cmp(other)
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED> Hash
for Supercow<'a, OWNED, BORROWED, SHARED>
where BORROWED : 'a + Hash,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    fn hash<H : Hasher>(&self, h: &mut H) {
        (**self).hash(h)
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED> Borrow<BORROWED>
for Supercow<'a, OWNED, BORROWED, SHARED>
where BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    fn borrow(&self) -> &BORROWED {
        self.deref()
    }
}

impl<'a, OWNED, BORROWED : ?Sized, SHARED> AsRef<BORROWED>
for Supercow<'a, OWNED, BORROWED, SHARED>
where BORROWED : 'a,
      &'a BORROWED : PointerFirstRef,
      SHARED : ConstDeref<Target = BORROWED> {
    fn as_ref(&self) -> &BORROWED {
        self.deref()
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn ref_to_owned() {
        let x = 42u32;
        let a: Supercow<u32> = Supercow::borrowed(&x);
        assert_eq!(x, *a);
        assert_eq!(&x as *const u32 as usize,
                   (&*a) as *const u32 as usize);

        let mut b = a.clone();
        assert_eq!(x, *b);
        assert_eq!(&x as *const u32 as usize,
                   (&*b) as *const u32 as usize);

        *b.to_mut() = 56;
        assert_eq!(42, *a);
        assert_eq!(x, *a);
        assert_eq!(&x as *const u32 as usize,
                   (&*a) as *const u32 as usize);
        assert_eq!(56, *b);
    }

    #[test]
    fn supports_dst() {
        let a: Supercow<String, str> = Supercow::borrowed("hello");
        let b: Supercow<String, str> = Supercow::owned("hello".to_owned());
        assert_eq!(a, b);

        let mut c = a.clone();
        c.to_mut().push_str(" world");
        assert_eq!(a, b);
        assert_eq!(c, "hello world");
    }

    #[test]
    fn default_accepts_arc() {
        let x: Supercow<u32> = Supercow::shared(Arc::new(42u32));
        assert_eq!(42, *x);
    }
}
