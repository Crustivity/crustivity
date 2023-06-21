use std::mem::MaybeUninit;

const BUCKET_SIZE: usize = 1 << 8;

struct Bucket<T> {
    data: Box<[MaybeUninit<T>; BUCKET_SIZE]>,
    used: [u8; BUCKET_SIZE / (u8::BITS as usize)],
}

impl<T> Bucket<T> {
    fn new() -> Self {
        Self {
            data: Box::new(unsafe {
                MaybeUninit::<[MaybeUninit<T>; BUCKET_SIZE]>::uninit().assume_init()
            }),
            used: [0u8; BUCKET_SIZE / (u8::BITS as usize)],
        }
    }

    fn index_in_use(&self, bucket_index: usize) -> bool {
        let bit: u8 = 1 << (bucket_index % (u8::BITS as usize));
        let used_idx = bucket_index / (u8::BITS as usize);
        self.used
            .get(used_idx)
            .copied()
            .map(|used_byte| (used_byte & bit) != 0)
            .unwrap_or(false)
    }
}

#[derive(Clone, Copy)]
struct Association {
    bucket_idx: usize,
    full: bool,
}

pub(crate) struct SVec<T> {
    data: Vec<Bucket<T>>,
    association: Vec<Association>,
}

impl<T> SVec<T> {
    pub(crate) fn new() -> Self {
        Self {
            data: Vec::new(),
            association: Vec::new(),
        }
    }

    pub(crate) fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        let association = *self.association.get(index / BUCKET_SIZE)?;

        let bucket = self.data.get_mut(association.bucket_idx)?;

        if !bucket.index_in_use(index % BUCKET_SIZE) {
            return None;
        }

        let t = bucket.data.get_mut(index % BUCKET_SIZE)?;
        Some(unsafe { t.assume_init_mut() })
    }

    pub(crate) fn get(&self, index: usize) -> Option<&T> {
        let association = *self.association.get(index / BUCKET_SIZE)?;

        let bucket = self.data.get(association.bucket_idx)?;

        if !bucket.index_in_use(index % BUCKET_SIZE) {
            return None;
        }

        let t = bucket.data.get(index % BUCKET_SIZE)?;
        Some(unsafe { t.assume_init_ref() })
    }

    pub(crate) fn push(&mut self, t: T) -> usize {
        let association = self.association.iter_mut().find(|a| !a.full);
        if let Some(association) = association {
            // put data in free slot
            let bucket = self
                .data
                .get_mut(association.bucket_idx)
                .expect("association indices should always be valid");
            for (byte_idx, used_byte) in bucket.used.iter_mut().enumerate() {
                if *used_byte != u8::MAX {
                    for (bit_idx, bit) in (0..8).map(|bit_idx| (bit_idx, 1u8 << bit_idx)) {
                        if *used_byte & bit == 0 {
                            let entry = &mut bucket.data
                                [byte_idx * (u8::BITS as usize) + (bit_idx as usize)];
                            *used_byte |= bit;
                            entry.write(t);
                            if bucket.used[byte_idx..]
                                .iter()
                                .all(|&used_byte_tail| used_byte_tail == u8::MAX)
                            {
                                association.full = true;
                            }
                            return association.bucket_idx * BUCKET_SIZE
                                + byte_idx * (u8::BITS as usize)
                                + bit_idx;
                        }
                    }
                }
            }
            panic!("there to be one non full byte, since the association said it was not full");
        } else {
            // put data in new bucket
            let bucket_idx = self.data.len();
            let mut bucket = Bucket::new();
            bucket.data[0].write(t);
            bucket.used[0] = 1u8;
            self.data.push(bucket);
            self.association.push(Association {
                bucket_idx,
                full: false,
            });
            bucket_idx * BUCKET_SIZE
        }
    }

    pub(crate) fn remove(&mut self, index: usize) -> Option<T> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_one_item() {
        let mut v = SVec::new();
        v.push(42);
        assert_eq!(v.get(0).copied(), Some(42));
    }

    #[test]
    fn pushing_getting_1000_elems() {
        let mut v = SVec::new();
        for i in 0..1000 {
            assert_eq!(i as usize, v.push(i));
        }

        for i in 0usize..1000 {
            assert_eq!(v.get(i).copied(), Some(i as i32));
        }
    }

    #[test]
    fn push_mut_and_get() {
        let mut v = SVec::new();
        for i in 0..1000 {
            v.push(i);
        }

        for i in 0usize..1200 {
            if let Some(n) = v.get_mut(i) {
                *n += 1;
            }
        }

        for i in 0usize..1000 {
            assert_eq!(v.get(i).copied(), Some((i + 1) as i32));
        }
    }
}
