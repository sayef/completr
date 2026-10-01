//! Aligned little-endian sections, read in place from a buffer or memory map.

use std::marker::PhantomData;
use std::sync::Arc;

use crate::Error;

#[cfg(target_endian = "big")]
compile_error!("completr segments are little-endian and read in place");

const ALIGN: usize = 8;

/// A shared, cheaply cloned byte range, backed by a buffer or a memory map.
#[derive(Clone)]
pub(crate) struct Bytes {
    owner: Arc<dyn AsRef<[u8]> + Send + Sync>,
    /// The range within `owner`'s bytes, which never move or change.
    ptr: *const u8,
    len: usize,
}

// SAFETY: `ptr` points into the immutable bytes of `owner`, which is `Send + Sync`.
unsafe impl Send for Bytes {}
// SAFETY: as above.
unsafe impl Sync for Bytes {}

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
        let data = (*owner).as_ref();
        let (ptr, len) = (data.as_ptr(), data.len());
        Self { owner, ptr, len }
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
        let part = &self.as_ref()[start..end];
        Self {
            owner: self.owner.clone(),
            ptr: part.as_ptr(),
            len: part.len(),
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
    #[inline]
    fn as_ref(&self) -> &[u8] {
        // SAFETY: `ptr` and `len` came from a slice of `owner`, which this keeps alive.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
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

/// Where a spilling [`Writer`] passes its bytes on.
pub(crate) type Spill<'s> = &'s mut dyn FnMut(&[u8]) -> Result<(), Error>;

/// Bytes a spilling writer holds before passing them on.
const SPILL_BYTES: usize = 1 << 20;

/// Little-endian, aligned output: kept in `buf`, or passed on to a spill about a megabyte at a time
/// so a large section is never held whole.
#[derive(Default)]
pub(crate) struct Writer<'s> {
    pub(crate) buf: Vec<u8>,
    /// Bytes already passed on.
    flushed: usize,
    spill: Option<Spill<'s>>,
    error: Option<Error>,
}

impl<'s> Writer<'s> {
    pub(crate) fn spilling(spill: Spill<'s>) -> Self {
        Self {
            spill: Some(spill),
            ..Self::default()
        }
    }

    fn flush(&mut self) {
        if let Some(spill) = &mut self.spill {
            if !self.buf.is_empty() && self.error.is_none() {
                if let Err(e) = spill(&self.buf) {
                    self.error = Some(e);
                }
            }
            self.flushed += self.buf.len();
            self.buf.clear();
        }
    }

    fn push(&mut self, v: &[u8]) {
        if self.spill.is_some() && v.len() >= SPILL_BYTES {
            self.flush();
            if let (Some(spill), None) = (&mut self.spill, &self.error) {
                if let Err(e) = spill(v) {
                    self.error = Some(e);
                }
            }
            self.flushed += v.len();
            return;
        }
        self.buf.extend_from_slice(v);
        if self.spill.is_some() && self.buf.len() >= SPILL_BYTES {
            self.flush();
        }
    }

    /// Passes on what is left; the first error of any spill.
    pub(crate) fn finish(mut self) -> Result<(), Error> {
        self.flush();
        self.error.map_or(Ok(()), Err)
    }

    pub(crate) fn raw(&mut self, v: &[u8]) {
        self.push(v);
    }

    pub(crate) fn u64(&mut self, v: u64) {
        self.push(&v.to_le_bytes());
    }

    pub(crate) fn align(&mut self) {
        let pos = self.flushed + self.buf.len();
        self.push(&[0; ALIGN][..pos.next_multiple_of(ALIGN) - pos]);
    }

    /// Length-prefixed bytes, padded to the alignment.
    pub(crate) fn bytes(&mut self, v: &[u8]) {
        self.align();
        self.u64(v.len() as u64);
        self.push(v);
        self.align();
    }

    /// An element count and the elements, aligned for in-place reads.
    pub(crate) fn column<T: Pod>(&mut self, v: &[T]) {
        self.align();
        self.u64(v.len() as u64);
        self.push(as_bytes(v));
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
            return Err(Error::Corrupt(
                "segment buffer is not 8-byte aligned".into(),
            ));
        }
        Ok(Self { data, pos: 0 })
    }

    fn take(&mut self, n: usize) -> Result<(usize, usize), Error> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.as_ref().len());
        let end = end.ok_or_else(|| Error::Corrupt("truncated segment".into()))?;
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
            usize::try_from(self.u64()?).map_err(|_| Error::Corrupt("length overflow".into()))?;
        let bytes = n
            .checked_mul(width)
            .ok_or_else(|| Error::Corrupt("length overflow".into()))?;
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
            Err(Error::Corrupt("trailing bytes in segment".into()))
        }
    }
}
