/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::{
    any::{Any, TypeId},
    borrow::Cow,
    collections::VecDeque,
    marker::PhantomData,
};

use dashmap::DashMap;
use parking_lot::Mutex;

pub trait Component: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Component for T {}

pub(crate) struct AnyType(pub(crate) Box<dyn Any + Send + Sync>);

pub(crate) struct ActivationEntry(pub(crate) fn(world: &World));

pub struct Variable<T> {
    pub(crate) index: usize,
    pub(crate) _t: PhantomData<T>,
}

impl<T> Clone for Variable<T> {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            _t: PhantomData,
        }
    }
}

impl<T> Copy for Variable<T> {}

#[derive(Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub struct VariableDyn {
    pub(crate) index: usize,
    pub(crate) tid: TypeId,
}

pub(crate) enum DynVarCommand<'a> {
    GetName(&'a mut dyn FnMut(Option<Cow<'static, str>>)),
}

pub(crate) struct AnyVar {
    pub(crate) any: AnyType,
    pub(crate) dyn_caller: fn(DynVarCommand, VariableDyn, &World),
}

pub trait TaskDataInternal: Clone + Component {
    fn register_vars(&self, register: &mut dyn VariableRegister);
}

pub trait TaskData: 'static {
    type Vars: TaskDataInternal;
}

pub struct Task<D: TaskData> {
    pub(crate) index: usize,
    pub(crate) _t: PhantomData<D::Vars>,
}

impl<D: TaskData> Clone for Task<D> {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            _t: PhantomData,
        }
    }
}

impl<D: TaskData> Copy for Task<D> {}

#[derive(Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub struct TaskDyn {
    pub(crate) index: usize,
    pub(crate) tid: TypeId,
}

pub enum TaskParameter<T> {
    Variable(Variable<T>),
    Event,
    Effect,
    Resource,
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub enum TaskParameterDyn {
    Variable(VariableDyn),
    Event(TypeId),
    Effect(TypeId),
    Resource(TypeId),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKindVariant {
    Ref,
    Mut,
}

pub trait VariableRegister {
    fn register_var(&mut self, ref_kind: RefKindVariant, var: TaskParameterDyn);
}

pub(crate) enum DynTaskCommand<'a> {
    Call,
    ListVars(&'a mut dyn VariableRegister),
    GetName(&'a mut dyn FnMut(Option<Cow<'static, str>>)),
}

pub(crate) struct AnyTask {
    pub(crate) any: AnyType,
    pub(crate) dyn_caller: fn(DynTaskCommand, TaskDyn, &World),
}

pub(crate) struct AnyEvent {
    pub(crate) any: AnyType,
    pub(crate) name: Option<Cow<'static, str>>,
}

pub(crate) struct AnyEffect {
    pub(crate) any: AnyType,
    pub(crate) name: Option<Cow<'static, str>>,
    pub(crate) finisher: fn(&World),
}

pub(crate) struct AnyResource {
    pub(crate) any: AnyType,
    pub(crate) name: Option<Cow<'static, str>>,
}

pub struct World {
    pub(crate) vars: DashMap<TypeId, AnyVar>,
    pub(crate) tasks: DashMap<TypeId, AnyTask>,

    pub(crate) events: DashMap<TypeId, AnyEvent>,
    pub(crate) effects: DashMap<TypeId, AnyEffect>,
    pub(crate) resources: DashMap<TypeId, AnyResource>,

    pub(crate) activations: Mutex<VecDeque<ActivationEntry>>,
    pub(crate) insertion_lock: Mutex<()>,
}
