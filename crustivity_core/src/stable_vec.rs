/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::mem::MaybeUninit;

const BUCKET_SIZE: usize = 1 << 8;

struct Bucket<T> {
    data: Box<[MaybeUninit<T>; BUCKET_SIZE]>,
    used: [u8; BUCKET_SIZE / (u8::BITS as usize)],
}

impl<T> Bucket<T> {
    fn new() -> Self {
        Self {
            data: Box::new(
                // SAFETY: `MaybeUninit::uninit_array` does the same thing. does the same thing.
                #[allow(clippy::uninit_assumed_init)]
                unsafe {
                    MaybeUninit::<[MaybeUninit<T>; BUCKET_SIZE]>::uninit().assume_init()
                },
            ),
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AssociationState {
    Full,
    Hollow,
    Disabled,
}

impl AssociationState {
    fn get_ascci_byte(self) -> u8 {
        match self {
            AssociationState::Full => b'F',
            AssociationState::Hollow => b'H',
            AssociationState::Disabled => b'D',
        }
    }
}

#[derive(Clone, Copy)]
struct Association {
    bucket_idx: usize,
    state: AssociationState,
}

trait AssociationExt {
    fn get_valid(&self, index: usize) -> Option<&Association>;
    fn mut_valid(&mut self, index: usize) -> Option<&mut Association>;
}

impl AssociationExt for Vec<Association> {
    fn get_valid(&self, index: usize) -> Option<&Association> {
        self.get(index).and_then(|association| {
            (association.state != AssociationState::Disabled).then_some(association)
        })
    }

    fn mut_valid(&mut self, index: usize) -> Option<&mut Association> {
        self.get_mut(index).and_then(|association| {
            (association.state != AssociationState::Disabled).then_some(association)
        })
    }
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
        let association = *self.association.get_valid(index / BUCKET_SIZE)?;

        let bucket = self.data.get_mut(association.bucket_idx)?;

        if !bucket.index_in_use(index % BUCKET_SIZE) {
            return None;
        }

        let t = bucket.data.get_mut(index % BUCKET_SIZE)?;
        Some(unsafe { t.assume_init_mut() })
    }

    pub(crate) fn get(&self, index: usize) -> Option<&T> {
        let association = *self.association.get_valid(index / BUCKET_SIZE)?;

        let bucket = self.data.get(association.bucket_idx)?;

        if !bucket.index_in_use(index % BUCKET_SIZE) {
            return None;
        }

        let t = bucket.data.get(index % BUCKET_SIZE)?;
        Some(unsafe { t.assume_init_ref() })
    }

    pub(crate) fn push(&mut self, t: T) -> usize {
        let association = self
            .association
            .iter_mut()
            .find(|a| a.state == AssociationState::Hollow);
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
                            let entry = &mut bucket.data[byte_idx * (u8::BITS as usize) + bit_idx];
                            *used_byte |= bit;
                            entry.write(t);
                            if bucket.used[byte_idx..]
                                .iter()
                                .all(|&used_byte_tail| used_byte_tail == u8::MAX)
                            {
                                association.state = AssociationState::Full;
                            }
                            return association.bucket_idx * BUCKET_SIZE
                                + byte_idx * (u8::BITS as usize)
                                + bit_idx;
                        }
                    }
                }
            }
            panic!("there to be one non full byte, since the association said it was not full");
        } else if let Some(association) = self
            .association
            .iter_mut()
            .find(|a| a.state == AssociationState::Disabled)
        {
            let bucket_idx = self.data.len();
            let mut bucket = Bucket::new();
            bucket.data[0].write(t);
            bucket.used[0] = 1u8;
            self.data.push(bucket);
            association.state = AssociationState::Hollow;
            association.bucket_idx = bucket_idx;
            bucket_idx * BUCKET_SIZE
        } else {
            // put data in new bucket
            let bucket_idx = self.data.len();
            let mut bucket = Bucket::new();
            bucket.data[0].write(t);
            bucket.used[0] = 1u8;
            self.data.push(bucket);
            self.association.push(Association {
                bucket_idx,
                state: AssociationState::Hollow,
            });
            bucket_idx * BUCKET_SIZE
        }
    }

    pub(crate) fn remove(&mut self, index: usize) -> Option<T> {
        let association = self.association.mut_valid(index / BUCKET_SIZE)?;

        let bucket = self.data.get_mut(association.bucket_idx)?;
        let bucket_index = index % BUCKET_SIZE;
        let bit: u8 = 1 << (bucket_index % (u8::BITS as usize));
        let used_idx = bucket_index / (u8::BITS as usize);

        let used_byte = bucket.used.get_mut(used_idx)?;

        if (*used_byte & bit) == 0 {
            return None;
        }

        *used_byte &= !bit;
        let t = bucket.data.get_mut(bucket_index).map(|t| {
            let mut uninit = MaybeUninit::uninit();
            std::mem::swap(t, &mut uninit);
            unsafe { uninit.assume_init() }
        });

        association.state = if *used_byte == u8::MAX {
            AssociationState::Full
        } else {
            AssociationState::Hollow
        };
        if bucket.used.iter().all(|used_byte| *used_byte == 0) {
            // bucket free -> remove bucket

            let last_idx = self.data.len() - 1;
            let asso_bucket_idx = association.bucket_idx;
            association.state = AssociationState::Disabled;
            let _ = self.data.swap_remove(asso_bucket_idx);
            self.association
                .iter_mut()
                .find(|asso| asso.bucket_idx == last_idx)
                .unwrap()
                .bucket_idx = asso_bucket_idx;
        }

        t
    }

    #[allow(dead_code)]
    fn print_meta_info(&self) {
        println!("buckets: {}", self.data.len());
        println!(
            "associations: {}, {}",
            self.association.len(),
            self.association_state()
        );
        for b in &self.data {
            for byte in b.used {
                let str_repr: Vec<_> = (0usize..8)
                    .map(|bit_idx| byte & (1u8 << bit_idx) != 0)
                    .map(|set| if set { b'|' } else { b'_' })
                    .collect();
                let str_repr = String::from_utf8_lossy(&str_repr);
                print!("{str_repr}");
            }
            println!();
        }
    }

    #[allow(dead_code)]
    fn association_state(&self) -> String {
        String::from_utf8_lossy(
            &self
                .association
                .iter()
                .map(|a| a.state.get_ascci_byte())
                .collect::<Vec<_>>(),
        )
        .into_owned()
    }
}

impl<T> Drop for SVec<T> {
    fn drop(&mut self) {
        for association in self
            .association
            .iter()
            .filter(|a| a.state != AssociationState::Disabled)
        {
            let Some(bucket) = self.data.get_mut(association.bucket_idx) else {continue;};
            match association.state {
                AssociationState::Disabled => unreachable!(),
                AssociationState::Full => {
                    for item in &mut *bucket.data {
                        unsafe { item.assume_init_drop() };
                    }
                }
                AssociationState::Hollow => {
                    for (byte_idx, byte) in bucket
                        .used
                        .iter()
                        .copied()
                        .enumerate()
                        .filter(|(_, byte)| *byte != 0)
                    {
                        for index in (0usize..8)
                            .filter(|bit_idx| (1u8 << bit_idx) & byte != 0)
                            .map(|bit| byte_idx * (u8::BITS as usize) + bit)
                        {
                            let Some(item) = bucket.data.get_mut(index) else {continue;};
                            unsafe { item.assume_init_drop() };
                        }
                    }
                }
            }
        }
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

    #[test]
    fn push_and_remove() {
        let mut v = SVec::new();
        for i in 0..1000 {
            v.push(i);
        }
        assert_eq!("FFFH", v.association_state());
        for i in 0..1000 {
            assert!(v.remove(i).is_some());
        }
        assert_eq!("DDDD", v.association_state());

        for i in 0..1000 {
            v.push(i);
        }
        assert_eq!("FFFH", v.association_state());
    }
}
