use crate::{
    database::Database,
    error::{
        mdbx_result,
        Error,
        Result,
    },
    flags::*,
    transaction::Transaction,
    util::freeze_bytes,
};
use libc::{
    c_uint,
    c_void,
};
use lifetimed_bytes::Bytes;
use std::{
    fmt,
    marker::PhantomData,
    mem,
    ptr,
    result,
};

/// An LMDB cursor.
pub trait Cursor<'txn, 'db, Txn> {
    /// Returns a raw pointer to the underlying LMDB cursor.
    ///
    /// The caller **must** ensure that the pointer is not used after the
    /// lifetime of the cursor.
    fn cursor(&self) -> *mut ffi::MDBX_cursor;

    fn dbi(&self) -> &'db Database<'txn, Txn>;

    /// Retrieves a key/data pair from the cursor. Depending on the cursor op,
    /// the current key may be returned.
    fn get(&self, key: Option<&[u8]>, data: Option<&[u8]>, op: c_uint) -> Result<(Option<Bytes<'txn>>, Bytes<'txn>)> {
        unsafe {
            let mut key_val = slice_to_val(key);
            let mut data_val = slice_to_val(data);
            let key_ptr = key_val.iov_base;
            mdbx_result(ffi::mdbx_cursor_get(self.cursor(), &mut key_val, &mut data_val, op))?;
            let txn = ffi::mdbx_cursor_txn(self.cursor());
            let key_out = if key_ptr != key_val.iov_base {
                Some(freeze_bytes(txn, &key_val)?)
            } else {
                None
            };
            let data_out = freeze_bytes(txn, &data_val)?;

            Ok((key_out, data_out))
        }
    }

    /// Iterate over database items. The iterator will begin with item next
    /// after the cursor, and continue until the end of the database. For new
    /// cursors, the iterator will begin with the first item in the database.
    ///
    /// For databases with duplicate data items (`DatabaseFlags::DUP_SORT`), the
    /// duplicate data items of each key will be returned before moving on to
    /// the next key.
    fn iter(&mut self) -> Iter<'txn, 'db, '_, Txn, Self>
    where
        Self: Sized,
    {
        Iter::new(self, ffi::MDBX_NEXT, ffi::MDBX_NEXT)
    }

    /// Iterate over database items starting from the beginning of the database.
    ///
    /// For databases with duplicate data items (`DatabaseFlags::DUP_SORT`), the
    /// duplicate data items of each key will be returned before moving on to
    /// the next key.
    fn iter_start(&mut self) -> Iter<'txn, 'db, '_, Txn, Self>
    where
        Self: Sized,
    {
        Iter::new(self, ffi::MDBX_FIRST, ffi::MDBX_NEXT)
    }

    /// Iterate over database items starting from the given key.
    ///
    /// For databases with duplicate data items (`DatabaseFlags::DUP_SORT`), the
    /// duplicate data items of each key will be returned before moving on to
    /// the next key.
    fn iter_from<K>(&mut self, key: K) -> Iter<'txn, 'db, '_, Txn, Self>
    where
        K: AsRef<[u8]>,
        Self: Sized,
    {
        match self.get(Some(key.as_ref()), None, ffi::MDBX_SET_RANGE) {
            Ok(_) | Err(Error::NotFound) => (),
            Err(error) => return Iter::Err(error),
        };
        Iter::new(self, ffi::MDBX_GET_CURRENT, ffi::MDBX_NEXT)
    }

    /// Iterate over duplicate database items. The iterator will begin with the
    /// item next after the cursor, and continue until the end of the database.
    /// Each item will be returned as an iterator of its duplicates.
    fn iter_dup(&mut self) -> IterDup<'txn, 'db, '_, Txn, Self>
    where
        Self: Sized,
    {
        IterDup::new(self, ffi::MDBX_NEXT)
    }

    /// Iterate over duplicate database items starting from the beginning of the
    /// database. Each item will be returned as an iterator of its duplicates.
    fn iter_dup_start(&mut self) -> IterDup<'txn, 'db, '_, Txn, Self>
    where
        Self: Sized,
    {
        IterDup::new(self, ffi::MDBX_FIRST)
    }

    /// Iterate over duplicate items in the database starting from the given
    /// key. Each item will be returned as an iterator of its duplicates.
    fn iter_dup_from<K>(&mut self, key: K) -> IterDup<'txn, 'db, '_, Txn, Self>
    where
        K: AsRef<[u8]>,
        Self: Sized,
    {
        match self.get(Some(key.as_ref()), None, ffi::MDBX_SET_RANGE) {
            Ok(_) | Err(Error::NotFound) => (),
            Err(error) => return IterDup::Err(error),
        };
        IterDup::new(self, ffi::MDBX_GET_CURRENT)
    }

    /// Iterate over the duplicates of the item in the database with the given key.
    fn iter_dup_of<K>(&mut self, key: K) -> Iter<'txn, 'db, '_, Txn, Self>
    where
        K: AsRef<[u8]>,
        Self: Sized,
    {
        match self.get(Some(key.as_ref()), None, ffi::MDBX_SET) {
            Ok(_) => (),
            Err(Error::NotFound) => {
                self.get(None, None, ffi::MDBX_LAST).ok();
                return Iter::new(self, ffi::MDBX_NEXT, ffi::MDBX_NEXT);
            },
            Err(error) => return Iter::Err(error),
        };
        Iter::new(self, ffi::MDBX_GET_CURRENT, ffi::MDBX_NEXT_DUP)
    }
}

/// A read-only cursor for navigating the items within a database.
pub struct RoCursor<'txn, 'db, Txn> {
    cursor: *mut ffi::MDBX_cursor,
    db: &'db Database<'txn, Txn>,
}

impl<'txn, 'db, Txn> Cursor<'txn, 'db, Txn> for RoCursor<'txn, 'db, Txn> {
    fn cursor(&self) -> *mut ffi::MDBX_cursor {
        self.cursor
    }

    fn dbi(&self) -> &'db Database<'txn, Txn> {
        self.db
    }
}

impl<'txn, 'db, Txn> Clone for RoCursor<'txn, 'db, Txn>
where
    Txn: Transaction,
{
    fn clone(&self) -> Self {
        Self::new_at_position(self).unwrap()
    }
}

impl<'txn, 'db, Txn> fmt::Debug for RoCursor<'txn, 'db, Txn> {
    fn fmt(&self, f: &mut fmt::Formatter) -> result::Result<(), fmt::Error> {
        f.debug_struct("RoCursor").finish()
    }
}

impl<'txn, 'db, Txn> Drop for RoCursor<'txn, 'db, Txn> {
    fn drop(&mut self) {
        unsafe { ffi::mdbx_cursor_close(self.cursor) }
    }
}

impl<'txn, 'db, Txn> RoCursor<'txn, 'db, Txn>
where
    Txn: Transaction,
{
    /// Creates a new read-only cursor in the given database and transaction.
    /// Prefer using `Transaction::open_cursor`.
    pub(crate) fn new(db: &'db Database<'txn, Txn>) -> Result<Self> {
        let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
        unsafe {
            mdbx_result(ffi::mdbx_cursor_open(db.txn().txn(), db.dbi(), &mut cursor))?;
        }
        Ok(Self {
            cursor,
            db,
        })
    }

    pub(crate) fn new_at_position<C: Cursor<'txn, 'db, Txn>>(other: &C) -> Result<Self> {
        unsafe {
            let cursor = ffi::mdbx_cursor_create(ptr::null_mut());

            let res = ffi::mdbx_cursor_copy(other.cursor(), cursor);

            let s = Self {
                cursor,
                db: other.dbi(),
            };

            mdbx_result(res)?;

            Ok(s)
        }
    }
}

/// A read-write cursor for navigating items within a database.
pub struct RwCursor<'txn, 'db, Txn> {
    cursor: *mut ffi::MDBX_cursor,
    db: &'db Database<'txn, Txn>,
}

impl<'txn, 'db, Txn> Cursor<'txn, 'db, Txn> for RwCursor<'txn, 'db, Txn> {
    fn cursor(&self) -> *mut ffi::MDBX_cursor {
        self.cursor
    }

    fn dbi(&self) -> &'db Database<'txn, Txn> {
        self.db
    }
}

impl<'txn, 'db, Txn> fmt::Debug for RwCursor<'txn, 'db, Txn> {
    fn fmt(&self, f: &mut fmt::Formatter) -> result::Result<(), fmt::Error> {
        f.debug_struct("RwCursor").finish()
    }
}

impl<'txn, 'db, Txn> Drop for RwCursor<'txn, 'db, Txn> {
    fn drop(&mut self) {
        unsafe { ffi::mdbx_cursor_close(self.cursor) }
    }
}

impl<'txn, 'db, Txn> RwCursor<'txn, 'db, Txn>
where
    Txn: Transaction,
{
    /// Creates a new read-only cursor in the given database and transaction.
    /// Prefer using `RwTransaction::open_rw_cursor`.
    pub(crate) fn new(db: &'db Database<'txn, Txn>) -> Result<RwCursor<'txn, 'db, Txn>> {
        let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
        unsafe {
            mdbx_result(ffi::mdbx_cursor_open(db.txn().txn(), db.dbi(), &mut cursor))?;
        }
        Ok(Self {
            cursor,
            db,
        })
    }

    /// Puts a key/data pair into the database. The cursor will be positioned at
    /// the new data item, or on failure usually near it.
    pub fn put<K, D>(&mut self, key: &K, data: &D, flags: WriteFlags) -> Result<()>
    where
        K: AsRef<[u8]>,
        D: AsRef<[u8]>,
    {
        let key = key.as_ref();
        let data = data.as_ref();
        let key_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: key.len(),
            iov_base: key.as_ptr() as *mut c_void,
        };
        let mut data_val: ffi::MDBX_val = ffi::MDBX_val {
            iov_len: data.len(),
            iov_base: data.as_ptr() as *mut c_void,
        };
        mdbx_result(unsafe { ffi::mdbx_cursor_put(self.cursor(), &key_val, &mut data_val, flags.bits()) })?;

        Ok(())
    }

    /// Deletes the current key/data pair.
    ///
    /// ### Flags
    ///
    /// `WriteFlags::NO_DUP_DATA` may be used to delete all data items for the
    /// current key, if the database was opened with `DatabaseFlags::DUP_SORT`.
    pub fn del(&mut self, flags: WriteFlags) -> Result<()> {
        mdbx_result(unsafe { ffi::mdbx_cursor_del(self.cursor(), flags.bits()) })?;

        Ok(())
    }
}

unsafe fn slice_to_val(slice: Option<&[u8]>) -> ffi::MDBX_val {
    match slice {
        Some(slice) => ffi::MDBX_val {
            iov_len: slice.len(),
            iov_base: slice.as_ptr() as *mut c_void,
        },
        None => ffi::MDBX_val {
            iov_len: 0,
            iov_base: ptr::null_mut(),
        },
    }
}

/// An iterator over the key/value pairs in an MDBX database.
#[derive(Debug)]
pub enum IntoIter<'txn, 'db, Txn, C>
where
    C: Cursor<'txn, 'db, Txn>,
    'txn: 'db,
{
    /// An iterator that returns an error on every call to `Iter::next`.
    /// Cursor.iter*() creates an Iter of this type when MDBX returns an error
    /// on retrieval of a cursor.  Using this variant instead of returning
    /// an error makes Cursor.iter()* methods infallible, so consumers only
    /// need to check the result of Iter.next().
    Err(Error),

    /// An iterator that returns an Item on calls to `Iter::next`.
    /// The Item is a `Result`, so this variant
    /// might still return an error, if retrieval of the key/value pair
    /// fails for some reason.
    Ok {
        /// The LMDB cursor with which to iterate.
        cursor: C,

        /// The first operation to perform when the consumer calls `Iter::next`.
        op: ffi::MDBX_cursor_op,

        /// The next and subsequent operations to perform.
        next_op: ffi::MDBX_cursor_op,

        _marker: PhantomData<fn(&'txn (), &'db (), &Txn)>,
    },
}

impl<'txn, 'db, Txn, C: Cursor<'txn, 'db, Txn>> IntoIter<'txn, 'db, Txn, C> {
    /// Creates a new iterator backed by the given cursor.
    fn new(cursor: C, op: ffi::MDBX_cursor_op, next_op: ffi::MDBX_cursor_op) -> Self {
        IntoIter::Ok {
            cursor,
            op,
            next_op,
            _marker: PhantomData,
        }
    }
}

impl<'txn, 'db, Txn, C: Cursor<'txn, 'db, Txn>> Iterator for IntoIter<'txn, 'db, Txn, C> {
    type Item = Result<(Bytes<'txn>, Bytes<'txn>)>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Ok {
                cursor,
                op,
                next_op,
                _marker,
            } => {
                let mut key = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let mut data = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let op = mem::replace(op, *next_op);
                unsafe {
                    match ffi::mdbx_cursor_get(cursor.cursor(), &mut key, &mut data, op) {
                        ffi::MDBX_SUCCESS => {
                            let txn = ffi::mdbx_cursor_txn(cursor.cursor());
                            let key = match freeze_bytes(txn, &key) {
                                Ok(v) => v,
                                Err(e) => return Some(Err(e)),
                            };
                            let data = match freeze_bytes(txn, &data) {
                                Ok(v) => v,
                                Err(e) => return Some(Err(e)),
                            };
                            Some(Ok((key, data)))
                        },
                        // EINVAL can occur when the cursor was previously seeked to a non-existent value,
                        // e.g. iter_from with a key greater than all values in the database.
                        ffi::MDBX_NOTFOUND | ffi::MDBX_ENODATA => None,
                        error => Some(Err(Error::from_err_code(error))),
                    }
                }
            },
            Self::Err(err) => Some(Err(*err)),
        }
    }
}

/// An iterator over the key/value pairs in an MDBX database.
#[derive(Debug)]
pub enum Iter<'txn, 'db, 'cur, Txn, C: Cursor<'txn, 'db, Txn>> {
    /// An iterator that returns an error on every call to `Iter::next`.
    /// Cursor.iter*() creates an Iter of this type when MDBX returns an error
    /// on retrieval of a cursor.  Using this variant instead of returning
    /// an error makes Cursor.iter()* methods infallible, so consumers only
    /// need to check the result of Iter.next().
    Err(Error),

    /// An iterator that returns an Item on calls to `Iter::next`.
    /// The Item is a `Result`, so this variant
    /// might still return an error, if retrieval of the key/value pair
    /// fails for some reason.
    Ok {
        /// The LMDB cursor with which to iterate.
        cursor: &'cur mut C,

        /// The first operation to perform when the consumer calls `Iter::next`.
        op: ffi::MDBX_cursor_op,

        /// The next and subsequent operations to perform.
        next_op: ffi::MDBX_cursor_op,

        _marker: PhantomData<fn(&'txn (), &'db (), &Txn)>,
    },
}

impl<'txn, 'db, 'cur, Txn, C: Cursor<'txn, 'db, Txn>> Iter<'txn, 'db, 'cur, Txn, C> {
    /// Creates a new iterator backed by the given cursor.
    fn new(cursor: &'cur mut C, op: ffi::MDBX_cursor_op, next_op: ffi::MDBX_cursor_op) -> Self {
        Iter::Ok {
            cursor,
            op,
            next_op,
            _marker: PhantomData,
        }
    }
}

impl<'txn, 'db, 'cur, Txn, C: Cursor<'txn, 'db, Txn>> Iterator for Iter<'txn, 'db, 'cur, Txn, C> {
    type Item = Result<(Bytes<'txn>, Bytes<'txn>)>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Iter::Ok {
                cursor,
                op,
                next_op,
                _marker,
            } => {
                let mut key = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let mut data = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let op = mem::replace(op, *next_op);
                unsafe {
                    match ffi::mdbx_cursor_get(cursor.cursor(), &mut key, &mut data, op) {
                        ffi::MDBX_SUCCESS => {
                            let txn = ffi::mdbx_cursor_txn(cursor.cursor());
                            let key = match freeze_bytes(txn, &key) {
                                Ok(v) => v,
                                Err(e) => return Some(Err(e)),
                            };
                            let data = match freeze_bytes(txn, &data) {
                                Ok(v) => v,
                                Err(e) => return Some(Err(e)),
                            };
                            Some(Ok((key, data)))
                        },
                        // EINVAL can occur when the cursor was previously seeked to a non-existent value,
                        // e.g. iter_from with a key greater than all values in the database.
                        ffi::MDBX_NOTFOUND | ffi::MDBX_ENODATA => None,
                        error => Some(Err(Error::from_err_code(error))),
                    }
                }
            },
            &mut Iter::Err(err) => Some(Err(err)),
        }
    }
}

/// An iterator over the keys and duplicate values in an LMDB database.
///
/// The yielded items of the iterator are themselves iterators over the duplicate values for a
/// specific key.
pub enum IterDup<'txn, 'db, 'cur, Txn, C: Cursor<'txn, 'db, Txn>> {
    /// An iterator that returns an error on every call to Iter.next().
    /// Cursor.iter*() creates an Iter of this type when LMDB returns an error
    /// on retrieval of a cursor.  Using this variant instead of returning
    /// an error makes Cursor.iter()* methods infallible, so consumers only
    /// need to check the result of Iter.next().
    Err(Error),

    /// An iterator that returns an Item on calls to Iter.next().
    /// The Item is a Result<(&'txn [u8], &'txn [u8])>, so this variant
    /// might still return an error, if retrieval of the key/value pair
    /// fails for some reason.
    Ok {
        /// The LMDB cursor with which to iterate.
        cursor: &'cur mut C,

        /// The first operation to perform when the consumer calls Iter.next().
        op: c_uint,

        _marker: PhantomData<fn(&'txn (), &'db (), &Txn)>,
    },
}

impl<'txn, 'db, 'cur, Txn, C: Cursor<'txn, 'db, Txn>> IterDup<'txn, 'db, 'cur, Txn, C> {
    /// Creates a new iterator backed by the given cursor.
    fn new(cursor: &'cur mut C, op: c_uint) -> Self {
        IterDup::Ok {
            cursor,
            op,
            _marker: PhantomData,
        }
    }
}

impl<'txn, 'db, 'cur, Txn, C: Cursor<'txn, 'db, Txn>> fmt::Debug for IterDup<'txn, 'db, 'cur, Txn, C> {
    fn fmt(&self, f: &mut fmt::Formatter) -> result::Result<(), fmt::Error> {
        f.debug_struct("IterDup").finish()
    }
}

impl<'txn, 'db, 'cur, Txn, C> Iterator for IterDup<'txn, 'db, 'cur, Txn, C>
where
    C: Cursor<'txn, 'db, Txn>,
    Txn: Transaction,
    Txn: 'txn,
    Txn: 'db,
    'txn: 'db,
    'txn: 'cur,
    'db: 'cur,
{
    type Item = IntoIter<'txn, 'db, Txn, RoCursor<'txn, 'db, Txn>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            IterDup::Ok {
                cursor,
                op,
                _marker,
            } => {
                let mut key = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let mut data = ffi::MDBX_val {
                    iov_len: 0,
                    iov_base: ptr::null_mut(),
                };
                let op = mem::replace(op, ffi::MDBX_NEXT_NODUP);
                let err_code = unsafe { ffi::mdbx_cursor_get(cursor.cursor(), &mut key, &mut data, op) };

                if err_code == ffi::MDBX_SUCCESS {
                    Some(IntoIter::new(
                        RoCursor::new_at_position(&**cursor).unwrap(),
                        ffi::MDBX_GET_CURRENT,
                        ffi::MDBX_NEXT_DUP,
                    ))
                } else {
                    None
                }
            },
            IterDup::Err(err) => Some(IntoIter::Err(*err)),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::environment::*;
    use ffi::*;
    use tempfile::tempdir;

    #[test]
    fn test_get() {
        let dir = tempdir().unwrap();
        let env = Environment::new().open(dir.path()).unwrap();

        let txn = env.begin_rw_txn().unwrap();
        let db = txn.open_db(None).unwrap();

        db.put(b"key1", b"val1", WriteFlags::empty()).unwrap();
        db.put(b"key2", b"val2", WriteFlags::empty()).unwrap();
        db.put(b"key3", b"val3", WriteFlags::empty()).unwrap();

        let cursor = db.open_ro_cursor().unwrap();
        assert_eq!((Some(b"key1".into()), b"val1".into()), cursor.get(None, None, MDBX_FIRST).unwrap());
        assert_eq!((Some(b"key1".into()), b"val1".into()), cursor.get(None, None, MDBX_GET_CURRENT).unwrap());
        assert_eq!((Some(b"key2".into()), b"val2".into()), cursor.get(None, None, MDBX_NEXT).unwrap());
        assert_eq!((Some(b"key1".into()), b"val1".into()), cursor.get(None, None, MDBX_PREV).unwrap());
        assert_eq!((Some(b"key3".into()), b"val3".into()), cursor.get(None, None, MDBX_LAST).unwrap());
        assert_eq!((None, b"val2".into()), cursor.get(Some(b"key2"), None, MDBX_SET).unwrap());
        assert_eq!((Some(b"key3".into()), b"val3".into()), cursor.get(Some(b"key3"), None, MDBX_SET_KEY).unwrap());
        assert_eq!((Some(b"key3".into()), b"val3".into()), cursor.get(Some(b"key2\0"), None, MDBX_SET_RANGE).unwrap());
    }

    #[test]
    fn test_get_dup() {
        let dir = tempdir().unwrap();
        let env = Environment::new().open(dir.path()).unwrap();

        let txn = env.begin_rw_txn().unwrap();
        let db = txn.create_db(None, DatabaseFlags::DUP_SORT).unwrap();
        db.put(b"key1", b"val1", WriteFlags::empty()).unwrap();
        db.put(b"key1", b"val2", WriteFlags::empty()).unwrap();
        db.put(b"key1", b"val3", WriteFlags::empty()).unwrap();
        db.put(b"key2", b"val1", WriteFlags::empty()).unwrap();
        db.put(b"key2", b"val2", WriteFlags::empty()).unwrap();
        db.put(b"key2", b"val3", WriteFlags::empty()).unwrap();

        let cursor = db.open_ro_cursor().unwrap();
        assert_eq!((Some(b"key1".into()), b"val1".into()), cursor.get(None, None, MDBX_FIRST).unwrap());
        assert_eq!((None, b"val1".into()), cursor.get(None, None, MDBX_FIRST_DUP).unwrap());
        assert_eq!((Some(b"key1".into()), b"val1".into()), cursor.get(None, None, MDBX_GET_CURRENT).unwrap());
        assert_eq!((Some(b"key2".into()), b"val1".into()), cursor.get(None, None, MDBX_NEXT_NODUP).unwrap());
        assert_eq!((Some(b"key2".into()), b"val2".into()), cursor.get(None, None, MDBX_NEXT_DUP).unwrap());
        assert_eq!((Some(b"key2".into()), b"val3".into()), cursor.get(None, None, MDBX_NEXT_DUP).unwrap());
        assert!(cursor.get(None, None, MDBX_NEXT_DUP).is_err());
        assert_eq!((Some(b"key2".into()), b"val2".into()), cursor.get(None, None, MDBX_PREV_DUP).unwrap());
        assert_eq!((None, b"val3".into()), cursor.get(None, None, MDBX_LAST_DUP).unwrap());
        assert_eq!((Some(b"key1".into()), b"val3".into()), cursor.get(None, None, MDBX_PREV_NODUP).unwrap());
        assert_eq!((None, b"val1".into()), cursor.get(Some(&b"key1"[..]), None, MDBX_SET).unwrap());
        assert_eq!((Some(b"key2".into()), b"val1".into()), cursor.get(Some(&b"key2"[..]), None, MDBX_SET_KEY).unwrap());
        assert_eq!(
            (Some(b"key2".into()), b"val1".into()),
            cursor.get(Some(&b"key1\0"[..]), None, MDBX_SET_RANGE).unwrap()
        );
        assert_eq!((None, b"val3".into()), cursor.get(Some(&b"key1"[..]), Some(&b"val3"[..]), MDBX_GET_BOTH).unwrap());
        assert_eq!(
            (None, b"val1".into()),
            cursor.get(Some(&b"key2"[..]), Some(&b"val"[..]), MDBX_GET_BOTH_RANGE).unwrap()
        );
    }

    #[test]
    fn test_get_dupfixed() {
        let dir = tempdir().unwrap();
        let env = Environment::new().open(dir.path()).unwrap();

        let txn = env.begin_rw_txn().unwrap();
        let db = txn.create_db(None, DatabaseFlags::DUP_SORT | DatabaseFlags::DUP_FIXED).unwrap();
        db.put(b"key1", b"val1", WriteFlags::empty()).unwrap();
        db.put(b"key1", b"val2", WriteFlags::empty()).unwrap();
        db.put(b"key1", b"val3", WriteFlags::empty()).unwrap();
        db.put(b"key2", b"val4", WriteFlags::empty()).unwrap();
        db.put(b"key2", b"val5", WriteFlags::empty()).unwrap();
        db.put(b"key2", b"val6", WriteFlags::empty()).unwrap();

        let cursor = db.open_ro_cursor().unwrap();
        assert_eq!((Some(b"key1".into()), b"val1".into()), cursor.get(None, None, MDBX_FIRST).unwrap());
        assert_eq!((None, b"val1val2val3".into()), cursor.get(None, None, MDBX_GET_MULTIPLE).unwrap());
        assert!(cursor.get(None, None, MDBX_NEXT_MULTIPLE).is_err());
    }

    #[test]
    fn test_iter() {
        let dir = tempdir().unwrap();
        let env = Environment::new().open(dir.path()).unwrap();

        let items: Vec<(Bytes, Bytes)> = vec![
            (b"key1".into(), b"val1".into()),
            (b"key2".into(), b"val2".into()),
            (b"key3".into(), b"val3".into()),
            (b"key5".into(), b"val5".into()),
        ];

        {
            let txn = env.begin_rw_txn().unwrap();
            let db = txn.open_db(None).unwrap();
            for &(ref key, ref data) in &items {
                db.put(key, data, WriteFlags::empty()).unwrap();
            }
            txn.commit().unwrap();
        }

        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        let mut cursor = db.open_ro_cursor().unwrap();

        // Because Result implements FromIterator, we can collect the iterator
        // of items of type Result<_, E> into a Result<Vec<_, E>> by specifying
        // the collection type via the turbofish syntax.
        assert_eq!(items, cursor.iter().collect::<Result<Vec<_>>>().unwrap());

        // Alternately, we can collect it into an appropriately typed variable.
        let retr: Result<Vec<_>> = cursor.iter_start().collect();
        assert_eq!(items, retr.unwrap());

        cursor.get(Some(b"key2"), None, MDBX_SET).unwrap();
        assert_eq!(
            items.clone().into_iter().skip(2).collect::<Vec<_>>(),
            cursor.iter().collect::<Result<Vec<_>>>().unwrap()
        );

        assert_eq!(items, cursor.iter_start().collect::<Result<Vec<_>>>().unwrap());

        assert_eq!(
            items.clone().into_iter().skip(1).collect::<Vec<_>>(),
            cursor.iter_from(b"key2").collect::<Result<Vec<_>>>().unwrap()
        );

        assert_eq!(
            items.clone().into_iter().skip(3).collect::<Vec<_>>(),
            cursor.iter_from(b"key4").collect::<Result<Vec<_>>>().unwrap()
        );

        assert_eq!(
            vec!().into_iter().collect::<Vec<(Bytes, Bytes)>>(),
            cursor.iter_from(b"key6").collect::<Result<Vec<_>>>().unwrap()
        );
    }

    #[test]
    fn test_iter_empty_database() {
        let dir = tempdir().unwrap();
        let env = Environment::new().open(dir.path()).unwrap();
        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        let mut cursor = db.open_ro_cursor().unwrap();

        assert_eq!(None, cursor.iter().next());
        assert_eq!(None, cursor.iter_start().next());
        assert_eq!(None, cursor.iter_from(b"foo").next());
    }

    #[test]
    fn test_iter_empty_dup_database() {
        let dir = tempdir().unwrap();
        let env = Environment::new().open(dir.path()).unwrap();

        let txn = env.begin_rw_txn().unwrap();
        txn.create_db(None, DatabaseFlags::DUP_SORT).unwrap();
        txn.commit().unwrap();

        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        let mut cursor = db.open_ro_cursor().unwrap();

        assert_eq!(None, cursor.iter().next());
        assert_eq!(None, cursor.iter_start().next());
        assert_eq!(None, cursor.iter_from(b"foo").next());
        assert_eq!(None, cursor.iter_from(b"foo").next());
        assert_eq!(None, cursor.iter_dup().flatten().next());
        assert_eq!(None, cursor.iter_dup_start().flatten().next());
        assert_eq!(None, cursor.iter_dup_from(b"foo").flatten().next());
        assert_eq!(None, cursor.iter_dup_of(b"foo").next());
    }

    #[test]
    fn test_iter_dup() {
        let dir = tempdir().unwrap();
        let env = Environment::new().open(dir.path()).unwrap();

        let txn = env.begin_rw_txn().unwrap();
        txn.create_db(None, DatabaseFlags::DUP_SORT).unwrap();
        txn.commit().unwrap();

        let items: Vec<(Bytes, Bytes)> = vec![
            (b"a".into(), b"1".into()),
            (b"a".into(), b"2".into()),
            (b"a".into(), b"3".into()),
            (b"b".into(), b"1".into()),
            (b"b".into(), b"2".into()),
            (b"b".into(), b"3".into()),
            (b"c".into(), b"1".into()),
            (b"c".into(), b"2".into()),
            (b"c".into(), b"3".into()),
            (b"e".into(), b"1".into()),
            (b"e".into(), b"2".into()),
            (b"e".into(), b"3".into()),
        ];

        {
            let txn = env.begin_rw_txn().unwrap();
            let db = txn.open_db(None).unwrap();
            for (key, data) in &items {
                db.put(key, data, WriteFlags::empty()).unwrap();
            }
            txn.commit().unwrap();
        }

        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        let mut cursor = db.open_ro_cursor().unwrap();
        assert_eq!(items, cursor.iter_dup().flatten().collect::<Result<Vec<_>>>().unwrap());

        cursor.get(Some(b"b"), None, MDBX_SET).unwrap();
        assert_eq!(
            items.clone().into_iter().skip(4).collect::<Vec<(Bytes, Bytes)>>(),
            cursor.iter_dup().flatten().collect::<Result<Vec<_>>>().unwrap()
        );

        assert_eq!(items, cursor.iter_dup_start().flatten().collect::<Result<Vec<(Bytes, Bytes)>>>().unwrap());

        assert_eq!(
            items.clone().into_iter().skip(3).collect::<Vec<(Bytes, Bytes)>>(),
            cursor.iter_dup_from(b"b").flatten().collect::<Result<Vec<_>>>().unwrap()
        );

        assert_eq!(
            items.clone().into_iter().skip(3).collect::<Vec<(Bytes, Bytes)>>(),
            cursor.iter_dup_from(b"ab").flatten().collect::<Result<Vec<_>>>().unwrap()
        );

        assert_eq!(
            items.clone().into_iter().skip(9).collect::<Vec<(Bytes, Bytes)>>(),
            cursor.iter_dup_from(b"d").flatten().collect::<Result<Vec<_>>>().unwrap()
        );

        assert_eq!(
            vec!().into_iter().collect::<Vec<(Bytes, Bytes)>>(),
            cursor.iter_dup_from(b"f").flatten().collect::<Result<Vec<_>>>().unwrap()
        );

        assert_eq!(
            items.clone().into_iter().skip(3).take(3).collect::<Vec<(Bytes, Bytes)>>(),
            cursor.iter_dup_of(b"b").collect::<Result<Vec<_>>>().unwrap()
        );

        assert_eq!(0, cursor.iter_dup_of(b"foo").count());
    }

    #[test]
    fn test_iter_del_get() {
        let dir = tempdir().unwrap();
        let env = Environment::new().open(dir.path()).unwrap();

        let items: Vec<(Bytes, Bytes)> = vec![(b"a".into(), b"1".into()), (b"b".into(), b"2".into())];
        let r: Vec<(_, _)> = Vec::new();
        {
            let txn = env.begin_rw_txn().unwrap();
            let db = txn.create_db(None, DatabaseFlags::DUP_SORT).unwrap();
            assert_eq!(r, db.open_ro_cursor().unwrap().iter_dup_of(b"a").collect::<Result<Vec<_>>>().unwrap());
            txn.commit().unwrap();
        }

        {
            let txn = env.begin_rw_txn().unwrap();
            let db = txn.open_db(None).unwrap();
            for (key, data) in &items {
                db.put(key, data, WriteFlags::empty()).unwrap();
            }
            txn.commit().unwrap();
        }

        let txn = env.begin_rw_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        let mut cursor = db.open_rw_cursor().unwrap();
        assert_eq!(items, cursor.iter_dup().flatten().collect::<Result<Vec<_>>>().unwrap());

        assert_eq!(
            items.clone().into_iter().take(1).collect::<Vec<(Bytes, Bytes)>>(),
            cursor.iter_dup_of(b"a").collect::<Result<Vec<_>>>().unwrap()
        );

        assert_eq!((None, b"1".into()), cursor.get(Some(b"a"), Some(b"1"), MDBX_SET).unwrap());

        cursor.del(WriteFlags::empty()).unwrap();

        assert_eq!(r, cursor.iter_dup_of(b"a").collect::<Result<Vec<_>>>().unwrap());
    }

    #[test]
    fn test_put_del() {
        let dir = tempdir().unwrap();
        let env = Environment::new().open(dir.path()).unwrap();

        let txn = env.begin_rw_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        let mut cursor = db.open_rw_cursor().unwrap();

        cursor.put(b"key1", b"val1", WriteFlags::empty()).unwrap();
        cursor.put(b"key2", b"val2", WriteFlags::empty()).unwrap();
        cursor.put(b"key3", b"val3", WriteFlags::empty()).unwrap();

        assert_eq!((Some(b"key3".into()), b"val3".into()), cursor.get(None, None, MDBX_GET_CURRENT).unwrap());

        cursor.del(WriteFlags::empty()).unwrap();
        assert_eq!((Some(b"key2".into()), b"val2".into()), cursor.get(None, None, MDBX_LAST).unwrap());
    }
}
