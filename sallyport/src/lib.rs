// SPDX-License-Identifier: Apache-2.0

//! This crate represents the hypervisor-microkernel boundary. It contains a number
//! of a shared structures to help facilitate communication between the two entities.

#![deny(missing_docs)]
#![deny(clippy::all)]
#![no_std]

//! The `proxy` module contains structures used to facilitate communication between
//! the microkernel and the hypervisor. This is referred to as "proxying" in the
//! project literature. This is a very thin and low-level layer that is meant to
//! be as transparent as possible.

use core::mem::size_of;
use memory::{Page, Register};

/// A request
///
/// The `Request` struct is the most minimal representation of the register context
/// needed for service requests from the microkernel to the hypervisor. An example
/// of such a request would be proxying a system call.
#[repr(C)]
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Request {
    /// The syscall number for the request
    ///
    /// See, for example, libc::SYS_exit.
    pub num: Register<usize>,

    /// The syscall argument registers
    ///
    /// At most 7 syscall arguments can be provided.
    pub arg: [Register<usize>; 7],
}

impl Request {
    /// Create a new request
    #[inline]
    pub fn new(num: impl Into<Register<usize>>, arg: &[Register<usize>]) -> Self {
        Self {
            num: num.into(),
            arg: [
                arg.get(0).copied().unwrap_or_default(),
                arg.get(1).copied().unwrap_or_default(),
                arg.get(2).copied().unwrap_or_default(),
                arg.get(3).copied().unwrap_or_default(),
                arg.get(4).copied().unwrap_or_default(),
                arg.get(5).copied().unwrap_or_default(),
                arg.get(6).copied().unwrap_or_default(),
            ],
        }
    }

    /// Issues the requested syscall and returns the reply
    ///
    /// # Safety
    ///
    /// This function is unsafe because syscalls can't be made generically safe.
    pub unsafe fn syscall(&self) -> Reply {
        extern "C" {
            fn sallyport_syscall(req: &Request, rep: &mut Reply);
        }

        let mut reply = core::mem::MaybeUninit::uninit().assume_init();
        sallyport_syscall(self, &mut reply);
        reply
    }
}

/// A reply
///
/// The `Reply` struct is the most minimal representation of the register context
/// needed for service replies from the hypervisor to the microkernel. An example
/// of such a reply would be the return value from a proxied system call.
///
/// Although most architectures collapse this to a single register value
/// with error numbers above `usize::max_value() - 4096`, `ppc64` uses
/// the `cr0.SO` flag to indicate error instead. Unfortunately, we also
/// can't use the built-in `Result` type for this, since its memory layout
/// is undefined. Therefore, we use this layout with conversions for `Result`.
#[repr(C)]
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Reply {
    ret: [Register<usize>; 2],
    err: Register<usize>,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
impl From<Result<[Register<usize>; 2], libc::c_int>> for Reply {
    #[inline]
    fn from(value: Result<[Register<usize>; 2], libc::c_int>) -> Self {
        match value {
            Ok(val) => Self {
                ret: val,
                err: Default::default(),
            },
            Err(val) => Self {
                ret: [(-val as usize).into(), Default::default()],
                err: Default::default(),
            },
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
impl From<Reply> for Result<[Register<usize>; 2], libc::c_int> {
    #[inline]
    fn from(value: Reply) -> Self {
        let reg: usize = value.ret[0].into();
        if reg > -4096isize as usize {
            Err(-(reg as libc::c_int))
        } else {
            Ok(value.ret)
        }
    }
}

/// A message, which is either a request or a reply
#[repr(C)]
#[derive(Copy, Clone)]
pub union Message {
    /// A request
    pub req: Request,

    /// A reply
    pub rep: Reply,
}

/// The `Block` struct encloses the Message's register contexts but also provides
/// a data buffer used to store data that might be required to service the request.
/// For example, bytes that must be written out by the host could be stored in the
/// `Block`'s `buf` field. It is expected that the trusted microkernel has copied
/// the necessary data components into the `Block`'s `buf` field and has updated
/// the `msg` register context fields accordingly in the event those registers
/// must point to those data components within the `buf`.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct Block {
    /// The register contexts for this message; either a request or a reply.
    pub msg: Message,

    /// A buffer where any additional request components may be stored. For example,
    /// a series of bytes to be written out in a proxied `write` syscall.
    ///
    /// Note that this buffer size is *less than* a page, since the buffer shares
    /// space with the `Message` that describes it.
    pub buf: [u8; Block::buf_capacity()],
}

impl Block {
    /// Returns the capacity of `Block.buf`
    pub const fn buf_capacity() -> usize {
        Page::size() - size_of::<Message>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_size() {
        assert_eq!(size_of::<Request>(), size_of::<usize>() * 8);
    }

    #[test]
    fn rep_size() {
        assert_eq!(size_of::<Reply>(), size_of::<usize>() * 3);
    }

    #[test]
    fn msg_size() {
        assert_eq!(size_of::<Message>(), size_of::<usize>() * 8);
    }

    #[test]
    fn block_size() {
        assert_eq!(size_of::<Block>(), Page::size());
    }

    #[test]
    fn syscall() {
        // Test syscall failure, including bidirectional conversion.
        let req = Request::new(libc::SYS_close, &[(-1isize as usize).into()]);
        let rep = unsafe { req.syscall() };
        assert_eq!(rep, Err(libc::EBADF).into());
        assert_eq!(libc::EBADF, Result::from(rep).unwrap_err());

        // Test dup() success.
        let req = Request::new(libc::SYS_dup, &[0usize.into()]);
        let rep = unsafe { req.syscall() };
        let res = Result::from(rep).unwrap()[0].into();
        assert_eq!(3usize, res);

        // Test close() success.
        let req = Request::new(libc::SYS_close, &[3usize.into()]);
        let rep = unsafe { req.syscall() };
        let res = Result::from(rep).unwrap()[0].into();
        assert_eq!(0usize, res);
    }
}
