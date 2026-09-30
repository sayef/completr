//! Aligned little-endian sections, read in place from a buffer or memory map.

use std::marker::PhantomData;
use std::sync::Arc;

use crate::Error;

#[cfg(target_endian = "big")]
compile_error!("strato segments are little-endian and read in place");

const ALIGN: usize = 8;

/// A shared, cheaply cloned byte range, backed by a buffer or a memory map.
#[derive(Clone)]
pub(crate) struct Bytes {
    owner: Arc<dyn AsRef<[u8]> + Send + Sync>,
    start: usize,
    end: usize,
}

/// A byte buffer whose start is 8-byte aligned.
struct Aligned {
    words: Vec<u64>,
    len: usize,
}

impl AsRef<[u8]> for Aligned {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: `words` holds at least `len` initialised bytes.
        unsafe { std::slice::from_raw_parts(self.words.as_ptr().cast::<u8>(), self.len) }
    }
}

impl Bytes {
    pub(crate) fn new(owner: Arc<dyn AsRef<[u8]> + Send + Sync>) -> Self {
        let end = (*owner).as_ref().len();
        Self {
            owner,
            start: 0,
            end,
        }
    }

    /// Keeps `data` if it is aligned, else copies it into an aligned buffer.
    pub(crate) fn from_vec(data: Vec<u8>) -> Self {
        if (data.as_ptr() as usize).is_multiple_of(ALIGN) {
            return Self::new(Arc::new(data));
        }
        let mut words = vec![0u64; data.len().div_ceil(ALIGN)];
        // SAFETY: `words` has room for `data.len()` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                words.as_mut_ptr().cast::<u8>(),
                data.len(),
            )
        };
        Self::new(Arc::new(Aligned {
            words,
            len: data.len(),
        }))
    }

    fn slice(&self, start: usize, end: usize) -> Self {
        Self {
            owner: self.owner.clone(),
            start: self.start + start,
            end: self.start + end,
        }
    }
}

impl Bytes {
    /// Reads one byte per page so the pages are mapped before queries need them.
    pub(crate) fn touch(&self) {
        let data = self.as_ref();
        let mut sum = 0u8;
        for i in (0..data.len()).step_by(4096) {
            // SAFETY: `i` is in bounds; volatile keeps the read from being optimised away.
            sum = sum.wrapping_add(unsafe { std::ptr::read_volatile(data.as_ptr().add(i)) });
        }
        std::hint::black_box(sum);
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &(*self.owner).as_ref()[self.start..self.end]
    }
}

/// Plain numeric types that are valid for any bit pattern.
pub(crate) trait Pod: Copy + 'static {}
impl Pod for u8 {}
impl Pod for u16 {}
impl Pod for u32 {}
impl Pod for u64 {}
impl Pod for f32 {}

/// A typed array read in place.
#[derive(Clone)]
pub(crate) struct Column<T> {
    bytes: Bytes,
    len: usize,
    _type: PhantomData<T>,
}

impl<T: Pod> Column<T> {
    pub(crate) fn as_slice(&self) -> &[T] {
        // SAFETY: the reader checked size and alignment, and `T` accepts any bit pattern.
        unsafe { std::slice::from_raw_parts(self.bytes.as_ref().as_ptr().cast::<T>(), self.len) }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }
}

fn as_bytes<T: Pod>(v: &[T]) -> &[u8] {
    // SAFETY: `T` is plain data without padding.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}

#[derive(Default)]
pub(crate) struct Writer {
    pub(crate) buf: Vec<u8>,
}

impl Writer {
    pub(crate) fn raw(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    pub(crate) fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub(crate) fn align(&mut self) {
        self.buf.resize(self.buf.len().next_multiple_of(ALIGN), 0);
    }

    /// Length-prefixed bytes, padded to the alignment.
    pub(crate) fn bytes(&mut self, v: &[u8]) {
        self.align();
        self.u64(v.len() as u64);
        self.buf.extend_from_slice(v);
        self.align();
    }

    /// An element count and the elements, aligned for in-place reads.
    pub(crate) fn column<T: Pod>(&mut self, v: &[T]) {
        self.align();
        self.u64(v.len() as u64);
        self.buf.extend_from_slice(as_bytes(v));
        self.align();
    }
}

pub(crate) struct Reader {
    data: Bytes,
    pos: usize,
}

impl Reader {
    pub(crate) fn new(data: Bytes) -> Result<Self, Error> {
        if !(data.as_ref().as_ptr() as usize).is_multiple_of(ALIGN) {
            return Err(Error::Format("segment buffer is not 8-byte aligned".into()));
        }
        Ok(Self { data, pos: 0 })
    }

    fn take(&mut self, n: usize) -> Result<(usize, usize), Error> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.as_ref().len());
        let end = end.ok_or_else(|| Error::Format("truncated segment".into()))?;
        let start = self.pos;
        self.pos = end;
        Ok((start, end))
    }

    pub(crate) fn raw(&mut self, n: usize) -> Result<&[u8], Error> {
        let (start, end) = self.take(n)?;
        Ok(&self.data.as_ref()[start..end])
    }

    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.raw(1)?[0])
    }

    pub(crate) fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.raw(8)?.try_into().unwrap()))
    }

    pub(crate) fn align(&mut self) -> Result<(), Error> {
        let padded = self
            .pos
            .next_multiple_of(ALIGN)
            .min(self.data.as_ref().len());
        self.take(padded - self.pos).map(|_| ())
    }

    fn count(&mut self, width: usize) -> Result<(usize, usize), Error> {
        self.align()?;
        let n =
            usize::try_from(self.u64()?).map_err(|_| Error::Format("length overflow".into()))?;
        let bytes = n
            .checked_mul(width)
            .ok_or_else(|| Error::Format("length overflow".into()))?;
        Ok((n, bytes))
    }

    pub(crate) fn bytes(&mut self) -> Result<Bytes, Error> {
        let (n, _) = self.count(1)?;
        let (start, end) = self.take(n)?;
        self.align()?;
        Ok(self.data.slice(start, end))
    }

    pub(crate) fn column<T: Pod>(&mut self) -> Result<Column<T>, Error> {
        let (len, size) = self.count(std::mem::size_of::<T>())?;
        let (start, end) = self.take(size)?;
        self.align()?;
        Ok(Column {
            bytes: self.data.slice(start, end),
            len,
            _type: PhantomData,
        })
    }

    pub(crate) fn finish(&self) -> Result<(), Error> {
        if self.pos == self.data.as_ref().len() {
            Ok(())
        } else {
            Err(Error::Format("trailing bytes in segment".into()))
        }
    }
}
