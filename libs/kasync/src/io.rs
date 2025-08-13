// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cmp;
use core::convert::Infallible;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll, ready};

pub trait Read {
    type Err: core::error::Error;

    // Pull some bytes from this source into the specified buffer.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, Self::Err>>;

    // Attempt to read from the AsyncRead into bufs using vectored IO operations.
    //
    // This method is similar to poll_read, but allows data to be read into multiple buffers using a single operation.
    //
    // On success, returns Poll::Ready(Ok(num_bytes_read)).
    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [&mut [u8]],
    ) -> Poll<Result<usize, Self::Err>> {
        let mut nread = 0;
        for b in bufs {
            if !b.is_empty() {
                nread += ready!(self.as_mut().poll_read(cx, b)?);
            }
        }
        Poll::Ready(Ok(nread))
    }
}

pub trait Write {
    type Err: core::error::Error;

    // Writes a buffer into this writer.
    //
    // returning how many bytes were written.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Self::Err>>;

    // Flushes this output stream, ensuring that all intermediately buffered contents reach their destination.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Err>>;

    // Attempt to close the object.
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Err>>;

    // Attempt to write bytes from bufs into the object using vectored IO operations.
    //
    // This method is similar to poll_write, but allows data from multiple buffers to be written using a single operation.
    //
    // On success, returns Poll::Ready(Ok(num_bytes_written)).
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[&[u8]],
    ) -> Poll<Result<usize, Self::Err>> {
        let mut nwritten = 0;
        for b in bufs {
            if !b.is_empty() {
                nwritten += ready!(self.as_mut().poll_write(cx, b)?);
            }
        }
        Poll::Ready(Ok(nwritten))
    }
}

// ===== impl Read =====

impl Read for &[u8] {
    type Err = Infallible;

    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, Self::Err>> {
        let amt = cmp::min(buf.len(), self.len());
        let (a, b) = self.split_at(amt);

        // First check if the amount of bytes we want to read is small:
        // `copy_from_slice` will generally expand to a call to `memcpy`, and
        // for a single byte the overhead is significant.
        if amt == 1 {
            buf[0] = a[0];
        } else {
            buf[..amt].copy_from_slice(a);
        }

        *self.get_mut() = b;

        Poll::Ready(Ok(amt))
    }
}

impl<P> Read for Pin<P>
where
    P: DerefMut + Unpin,
    <P as Deref>::Target: Read,
{
    type Err = <P::Target as Read>::Err;

    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, Self::Err>> {
        self.get_mut().as_mut().poll_read(cx, buf)
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [&mut [u8]],
    ) -> Poll<Result<usize, Self::Err>> {
        self.get_mut().as_mut().poll_read_vectored(cx, bufs)
    }
}

impl<T> Read for &mut T
where
    T: Read + Unpin + ?Sized,
{
    type Err = T::Err;

    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, Self::Err>> {
        Pin::new(&mut **self).poll_read(cx, buf)
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [&mut [u8]],
    ) -> Poll<Result<usize, Self::Err>> {
        Pin::new(&mut **self).poll_read_vectored(cx, bufs)
    }
}

// ===== impl Write =====

impl<P> Write for Pin<P>
where
    P: DerefMut + Unpin,
    <P as Deref>::Target: Write,
{
    type Err = <P::Target as Write>::Err;

    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Self::Err>> {
        self.get_mut().as_mut().poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Err>> {
        self.get_mut().as_mut().poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Err>> {
        self.get_mut().as_mut().poll_close(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[&[u8]],
    ) -> Poll<Result<usize, Self::Err>> {
        self.get_mut().as_mut().poll_write_vectored(cx, bufs)
    }
}

impl<T> Write for &mut T
where
    T: Write + Unpin + ?Sized,
{
    type Err = T::Err;

    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Self::Err>> {
        Pin::new(&mut **self).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Err>> {
        Pin::new(&mut **self).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Err>> {
        Pin::new(&mut **self).poll_close(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[&[u8]],
    ) -> Poll<Result<usize, Self::Err>> {
        Pin::new(&mut **self).poll_write_vectored(cx, bufs)
    }
}
