/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::{
    any::{Any, TypeId},
    collections::VecDeque,
};

use dashmap::DashMap;
use parking_lot::Mutex;

pub(crate) struct AnyType {
    pub(crate) any: Box<dyn Any + Send + Sync>,
}

pub(crate) struct ActivationEntry(pub(crate) fn(world: &World));

pub struct World {
    pub(crate) vars: DashMap<TypeId, AnyType>,
    pub(crate) events: DashMap<TypeId, AnyType>,
    pub(crate) tasks: DashMap<TypeId, AnyType>,
    pub(crate) resources: DashMap<TypeId, AnyType>,

    pub(crate) activations: Mutex<VecDeque<ActivationEntry>>,
}
