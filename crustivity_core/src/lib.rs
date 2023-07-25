/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::{
    any::{Any, TypeId},
    borrow::Cow,
    cell::UnsafeCell,
    collections::VecDeque,
    marker::PhantomData,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub mod constraints;
pub mod spawner;
mod world;

use crate::spawner::Spawner;
use crate::world::{
    ActivationEntry, AnyEffect, AnyEvent, AnyResource, AnyTask, AnyType, AnyVar, DynTaskCommand,
    DynVarCommand, RefKindVariant, TaskDataInternal, VariableRegister,
};
pub use crate::world::{
    Component, Task, TaskData, TaskDyn, TaskParameter, TaskParameterDyn, Variable, VariableDyn,
    World,
};
use constraints::{Constraint, ConstraintSystem, EffectPath};
use dashmap::DashMap;
use parking_lot::Mutex;
use sharded_slab::Slab;

trait MaybeUninitPtrTransmut {
    type Inner;

    fn inner_ptr(self) -> *mut Self::Inner;
}

impl<T> MaybeUninitPtrTransmut for *mut MaybeUninit<T> {
    type Inner = T;

    fn inner_ptr(self) -> *mut Self::Inner {
        unsafe { std::mem::transmute::<Self, *mut Self::Inner>(self) }
    }
}

trait ToSome: Sized {
    fn some(self) -> Option<Self>;
}

impl<T: Sized> ToSome for T {
    fn some(self) -> Option<Self> {
        Some(self)
    }
}

struct VarEntry<T> {
    t: UnsafeCell<T>,
    name: Option<Cow<'static, str>>,
}

struct VarTable<T> {
    data: Slab<VarEntry<T>>,
}

unsafe impl<T: Send> Sync for VarTable<T> {}

struct EventTable<T> {
    data: UnsafeCell<MaybeUninit<T>>,
    has_data: AtomicBool,
    activations: Mutex<VecDeque<T>>,
}

unsafe impl<T: Send> Sync for EventTable<T> {}

struct EffectTable<T> {
    data: UnsafeCell<T>,
    acivations: Mutex<Vec<T>>,
}

unsafe impl<T: Send> Sync for EffectTable<T> {}

struct TaskEntry<T> {
    t: UnsafeCell<T>,
    f: unsafe fn(),
    name: Option<Cow<'static, str>>,
}

struct TaskTable<T> {
    data: Slab<TaskEntry<T>>,
}

unsafe impl<T: Send> Sync for TaskTable<T> {}

struct ResourceTable<T> {
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for ResourceTable<T> {}

impl<T> VarTable<T> {
    fn new() -> Self {
        Self { data: Slab::new() }
    }
}

impl<T> EventTable<T> {
    fn new() -> Self {
        Self {
            data: UnsafeCell::new(MaybeUninit::uninit()),
            has_data: AtomicBool::new(false),
            activations: Mutex::new(VecDeque::new()),
        }
    }
}

impl<T: Default> EffectTable<T> {
    fn new() -> Self {
        Self {
            data: UnsafeCell::new(T::default()),
            acivations: Mutex::new(Vec::new()),
        }
    }
}

impl<T> TaskTable<T> {
    fn new() -> Self {
        Self { data: Slab::new() }
    }
}

impl<T> ResourceTable<T> {
    fn new(data: T) -> Self {
        Self {
            data: UnsafeCell::new(data),
        }
    }
}

fn variable_call_dyn<T: Component>(command: DynVarCommand, var: VariableDyn, world: &World) {
    let Some(var) = var.downcast::<T>() else {return;};
    match command {
        DynVarCommand::GetName(ret_name) => {
            ret_name(world.variable_name(var));
        }
    }
}

impl AnyVar {
    fn new<T: Component>() -> Self {
        Self {
            any: AnyType::new(VarTable::<T>::new()),
            dyn_caller: variable_call_dyn::<T>,
        }
    }
}

impl AnyTask {
    fn new<T: TaskData>(dyn_caller: fn(DynTaskCommand, TaskDyn, &World)) -> Self {
        Self {
            any: AnyType::new(TaskTable::<T::Vars>::new()),
            dyn_caller,
        }
    }
}

impl AnyEvent {
    fn new<T: Component>(name: Option<Cow<'static, str>>) -> Self {
        Self {
            any: AnyType::new(EventTable::<T>::new()),
            name,
        }
    }
}

impl AnyEffect {
    fn new<T: Component + Default>(name: Option<Cow<'static, str>>) -> Self {
        Self {
            any: AnyType::new(EffectTable::<T>::new()),
            name,
            finisher: World::finish_effect::<T>,
        }
    }
}

impl AnyResource {
    fn new<T: Component>(t: T, name: Option<Cow<'static, str>>) -> Self {
        Self {
            any: AnyType::new(ResourceTable::new(t)),
            name,
        }
    }
}

impl AnyType {
    fn new<G: Component>(any: G) -> Self {
        Self(Box::new(any) as Box<dyn Any + Send + Sync>)
    }

    fn get<TTable: Send + 'static>(&self, err_msg: &'static str) -> &TTable {
        self.0.downcast_ref::<TTable>().expect(err_msg)
    }
}

#[derive(Default)]
pub struct WorldBuilder {
    world: World,
}

pub trait WorldDataCreator {
    fn variable<T: Component>(&self, t: T) -> Variable<T>;

    fn variable_named<T: Component>(&self, t: T, name: impl Into<Cow<'static, str>>)
        -> Variable<T>;

    fn resource<T: Component>(&self, t: T) -> Result<(), T>;

    fn resource_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Result<(), T>;

    fn register_effect<T: Component + Default>(&self);

    fn register_effect_named<T: Component + Default>(&self, name: impl Into<Cow<'static, str>>);

    fn register_event<T: Component>(&self, t: T) -> Result<(), T>;

    fn register_event_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Result<(), T>;

    fn constraint<T: TaskData>(&self, task: Task<T>) -> Constraint;
}

pub trait WorldNameAccess {
    fn variable_name<T: Component>(&self, var: Variable<T>) -> Option<Cow<'static, str>>;
    fn variable_name_dyn(&self, var: VariableDyn) -> Option<Cow<'static, str>>;

    fn task_name<T: TaskData>(&self, task: Task<T>) -> Option<Cow<'static, str>>;
    fn task_name_dyn(&self, task: TaskDyn) -> Option<Cow<'static, str>>;

    fn event_name<T: Component>(&self) -> Option<Cow<'static, str>>;
    fn event_name_dyn(&self, event: TypeId) -> Option<Cow<'static, str>>;

    fn effect_name<T: Component>(&self) -> Option<Cow<'static, str>>;
    fn effect_name_dyn(&self, effect: TypeId) -> Option<Cow<'static, str>>;

    fn resource_name<T: Component>(&self) -> Option<Cow<'static, str>>;
    fn resource_name_dyn(&self, resource: TypeId) -> Option<Cow<'static, str>>;
}

impl WorldBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build(self, system: ConstraintSystem) -> Arc<World> {
        let world = Arc::new(self.world);
        let spawner = Spawner::new(Arc::clone(&world));
        world
            .resource(spawner)
            .ok()
            .expect("Spawner to be inserted");
        world
            .resource(system)
            .ok()
            .expect("ConstraintSystem to be inserted");
        world
    }
}

impl WorldDataCreator for WorldBuilder {
    fn variable<T: Component>(&self, t: T) -> Variable<T> {
        self.world.variable(t)
    }

    fn variable_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Variable<T> {
        self.world.variable_named(t, name)
    }

    fn resource<T: Component>(&self, t: T) -> Result<(), T> {
        self.world.resource(t)
    }

    fn resource_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Result<(), T> {
        self.world.resource_named(t, name)
    }

    fn register_effect<T: Component + Default>(&self) {
        self.world.register_effect::<T>()
    }

    fn register_effect_named<T: Component + Default>(&self, name: impl Into<Cow<'static, str>>) {
        self.world.register_effect_named::<T>(name)
    }

    fn register_event<T: Component>(&self, t: T) -> Result<(), T> {
        self.world.register_event(t)
    }

    fn register_event_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Result<(), T> {
        self.world.register_event_named(t, name)
    }

    fn constraint<T: TaskData>(&self, task: Task<T>) -> Constraint {
        self.world.constraint(task)
    }
}

impl WorldNameAccess for WorldBuilder {
    fn variable_name<T: Component>(&self, var: Variable<T>) -> Option<Cow<'static, str>> {
        self.world.variable_name(var)
    }

    fn variable_name_dyn(&self, var: VariableDyn) -> Option<Cow<'static, str>> {
        self.world.variable_name_dyn(var)
    }

    fn task_name<T: TaskData>(&self, task: Task<T>) -> Option<Cow<'static, str>> {
        self.world.task_name(task)
    }

    fn task_name_dyn(&self, task: TaskDyn) -> Option<Cow<'static, str>> {
        self.world.task_name_dyn(task)
    }

    fn event_name<T: Component>(&self) -> Option<Cow<'static, str>> {
        self.world.event_name::<T>()
    }

    fn event_name_dyn(&self, event: TypeId) -> Option<Cow<'static, str>> {
        self.world.event_name_dyn(event)
    }

    fn effect_name<T: Component>(&self) -> Option<Cow<'static, str>> {
        self.world.effect_name::<T>()
    }

    fn effect_name_dyn(&self, effect: TypeId) -> Option<Cow<'static, str>> {
        self.world.effect_name_dyn(effect)
    }

    fn resource_name<T: Component>(&self) -> Option<Cow<'static, str>> {
        self.world.resource_name::<T>()
    }

    fn resource_name_dyn(&self, resource: TypeId) -> Option<Cow<'static, str>> {
        self.world.resource_name_dyn(resource)
    }
}

impl World {
    fn insert_variable<T: Component>(&self, t: T, name: Option<Cow<'static, str>>) -> Variable<T> {
        let type_id = TypeId::of::<T>();
        let any = if let Some(any) = self.vars.get(&type_id) {
            any
        } else {
            let insertion_guard = self.insertion_lock.lock();
            if let Some(any) = self.vars.get(&type_id) {
                any
            } else {
                let previous = self.vars.insert(type_id, AnyVar::new::<T>());
                drop(insertion_guard);
                if previous.is_some() {
                    log::warn!("Insertions of new tasks, that were already present, should not happen and may result in undefined behaviour.");
                }
                self.vars.get(&type_id).unwrap()
            }
        };
        let table = any.any.get::<VarTable<T>>("any to be a VarTable<T>");
        Variable {
            index: table
                .data
                .insert(VarEntry {
                    t: UnsafeCell::new(t),
                    name,
                })
                .expect("out of memory"),
            _t: PhantomData,
        }
    }

    pub fn register_vars<T: TaskData>(&self, task: Task<T>, register: &mut dyn VariableRegister) {
        let Some(task) = self.task_fn(task).map(|(_, t)| t) else {return};
        task.register_vars(register);
    }

    pub fn register_vars_dyn(&self, task: TaskDyn, register: &mut dyn VariableRegister) {
        let Some(any) = self.tasks.get(&task.tid) else {return;};
        (any.dyn_caller)(DynTaskCommand::ListVars(register), task, self);
    }

    fn insert_task<T: TaskData>(
        &self,
        t: T::Vars,
        f: unsafe fn(),
        dyn_caller: fn(DynTaskCommand, TaskDyn, &World),
        name: Option<Cow<'static, str>>,
    ) -> Task<T> {
        let type_id = TypeId::of::<T>();
        let any = if let Some(any) = self.tasks.get(&type_id) {
            any
        } else {
            let insertion_guard = self.insertion_lock.lock();
            if let Some(any) = self.tasks.get(&type_id) {
                any
            } else {
                let previous = self.tasks.insert(type_id, AnyTask::new::<T>(dyn_caller));
                drop(insertion_guard);
                if previous.is_some() {
                    log::warn!("Insertions of new tasks, that were already present, should not happen and may result in undefined behaviour.");
                }
                self.tasks.get(&type_id).unwrap()
            }
        };
        let table = any
            .any
            .get::<TaskTable<T::Vars>>("any to be a TaskTable<T>");
        Task {
            index: table
                .data
                .insert(TaskEntry {
                    t: UnsafeCell::new(t),
                    f,
                    name,
                })
                .expect("out of memory"),
            _t: PhantomData::default(),
        }
    }

    fn task_fn<T: TaskData>(&self, task: Task<T>) -> Option<(unsafe fn(), T::Vars)> {
        let type_id = TypeId::of::<T>();
        self.tasks
            .get(&type_id)?
            .any
            .get::<TaskTable<T::Vars>>("any to be TaskTable<T::Vars>")
            .data
            .get(task.index)
            .map(|cell| (cell.f, Clone::clone(unsafe { &*cell.t.get() })))
    }

    pub fn task_call<T: TaskData>(&self, task: Task<T>) {
        let type_id = TypeId::of::<T>();
        let Some(any) = self.tasks.get(&type_id) else {return;};
        (any.dyn_caller)(DynTaskCommand::Call, task.erase(), self);
    }

    fn task_call_dyn(&self, task: TaskDyn) {
        let Some(any) = self.tasks.get(&task.tid) else {return;};
        (any.dyn_caller)(DynTaskCommand::Call, task, self);
    }

    fn insert_resource<T: Component>(
        &self,
        t: T,
        name: Option<Cow<'static, str>>,
    ) -> Result<(), T> {
        let type_id = TypeId::of::<T>();
        if self.resources.contains_key(&type_id) {
            Err(t)
        } else {
            let insertion_guard = self.insertion_lock.lock();
            if self.resources.contains_key(&type_id) {
                Err(t)
            } else {
                let previous = self.resources.insert(type_id, AnyResource::new(t, name));
                drop(insertion_guard);
                if previous.is_some() {
                    log::warn!("Insertions of new resources, that were already present, should not happen and may result in undefined behaviour.");
                }
                Ok(())
            }
        }
    }

    fn insert_effect<T: Component + Default>(&self, name: Option<Cow<'static, str>>) {
        let type_id = TypeId::of::<T>();
        if !self.effects.contains_key(&type_id) {
            let insertion_guard = self.insertion_lock.lock();
            if !self.effects.contains_key(&type_id) {
                let previous = self.effects.insert(type_id, AnyEffect::new::<T>(name));
                drop(insertion_guard);
                if previous.is_some() {
                    log::warn!("Insertions of new effects, that were already present, should not happen and may result in undefined behaviour.");
                }
            }
        }
    }

    pub fn receiv_effects<T: Component>(&self) -> Vec<T> {
        let type_id = TypeId::of::<T>();
        let Some(any) = self
            .effects
            .get(&type_id) else { return Vec::new();};
        let effect = any.any.get::<EffectTable<T>>("any to be EffectTable<T>");
        let mut activations = effect.acivations.lock();
        std::mem::take(&mut *activations)
    }

    fn insert_event<T: Component>(&self, t: T, name: Option<Cow<'static, str>>) -> Result<(), T> {
        let type_id = TypeId::of::<T>();
        if self.events.contains_key(&type_id) {
            Err(t)
        } else {
            let insertion_guard = self.insertion_lock.lock();
            if self.events.contains_key(&type_id) {
                Err(t)
            } else {
                let previous = self.events.insert(type_id, AnyEvent::new::<T>(name));
                let any = self.events.get(&type_id).unwrap();
                let event = any.any.get::<EventTable<T>>("any to be an EventTable<T>");
                let data_ptr = event.data.get();
                let data_ref = unsafe { &mut *data_ptr };
                data_ref.write(t);
                event.has_data.store(true, Ordering::Release);
                drop(insertion_guard);
                if previous.is_some() {
                    log::warn!("Insertions of new events, that were already present, should not happen and may result in undefined behaviour.");
                }
                Ok(())
            }
        }
    }

    pub fn emit_event<T: Component>(&self, t: T) {
        let type_id = TypeId::of::<T>();
        let any = if let Some(any) = self.events.get(&type_id) {
            any
        } else {
            let insertion_guard = self.insertion_lock.lock();
            if let Some(any) = self.events.get(&type_id) {
                any
            } else {
                let previous = self.events.insert(type_id, AnyEvent::new::<T>(None));
                drop(insertion_guard);
                if previous.is_some() {
                    log::warn!("Insertions of new events, that were already present, should not happen and may result in undefined behaviour.");
                }
                self.events.get(&type_id).unwrap()
            }
        };
        let event_table = any.any.get::<EventTable<T>>("any to be an EventTable<T>");
        event_table.activations.lock().push_back(t);

        self.activations
            .lock()
            .push_back(ActivationEntry(World::write_activation_and_emit::<T>));
    }

    fn write_activation_and_emit<T: Component>(&self) {
        let type_id = TypeId::of::<T>();
        let Some(any) = self.events.get(&type_id) else {return;};
        let event = any.any.get::<EventTable<T>>("any to be EventTable<T>");
        let Some(activation_value) = event.activations.lock().pop_front() else {return;};
        let data = event.data.get();
        // SAFETY: TODO: needs lock for concurrent usage
        let data = unsafe { &mut *data };
        if event.has_data.load(Ordering::Acquire) {
            unsafe { data.assume_init_drop() };
        }
        data.write(activation_value);
        event.has_data.store(true, Ordering::Release);

        let system = TaskParameter::<ConstraintSystem>::Resource
            .get_mut_ptr(self)
            .expect("ConstraintSystem to be an existing resource");

        // SAFETY: TODO: needs lock for concurrent usage
        let system = unsafe { &*system };
        if let Some(path) = EffectPath::starting_with(Event::<T>, system) {
            self.execute_effect_path(path);
        }
    }

    pub fn process_next_activation(&self) -> bool {
        let Some(activation) = self.activations.lock().pop_front() else {return false;};
        (activation.0)(self);
        true
    }

    fn finish_effect<T: Component + Default>(&self) {
        let type_id = TypeId::of::<T>();
        let Some(any) = self.effects.get(&type_id) else {return;};
        let effect = any.any.get::<EffectTable<T>>("any to be EffectTable<T>");
        let data_ptr = effect.data.get();
        // TODO: lock
        let data_ref = unsafe { &mut *data_ptr };
        let t = std::mem::take(data_ref);
        effect.acivations.lock().push(t);
    }

    fn execute_effect_path(&self, path: EffectPath) {
        for entry in path.tasks {
            self.task_call_dyn(entry);
        }
        for finisher in path
            .effects
            .iter()
            .flat_map(|tid| self.effects.get(tid).into_iter())
            .map(|any| any.finisher)
        {
            finisher(self);
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self {
            vars: DashMap::new(),
            tasks: DashMap::new(),

            events: DashMap::new(),
            effects: DashMap::new(),
            resources: DashMap::new(),

            activations: Mutex::new(VecDeque::new()),
            insertion_lock: Mutex::new(()),
        }
    }
}

impl WorldDataCreator for World {
    fn variable<T: Component>(&self, t: T) -> Variable<T> {
        self.insert_variable(t, None)
    }

    fn variable_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Variable<T> {
        self.insert_variable(t, name.into().some())
    }

    fn resource_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Result<(), T> {
        self.insert_resource(t, name.into().some())
    }

    fn resource<T: Component>(&self, t: T) -> Result<(), T> {
        self.insert_resource(t, None)
    }

    fn register_effect<T: Component + Default>(&self) {
        self.insert_effect::<T>(None)
    }

    fn register_effect_named<T: Component + Default>(&self, name: impl Into<Cow<'static, str>>) {
        self.insert_effect::<T>(name.into().some())
    }

    fn register_event<T: Component>(&self, t: T) -> Result<(), T> {
        self.insert_event(t, None)
    }

    fn register_event_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Result<(), T> {
        self.insert_event(t, name.into().some())
    }

    fn constraint<T: TaskData>(&self, task: Task<T>) -> Constraint {
        Constraint::new(task, self)
    }
}

impl WorldNameAccess for World {
    fn variable_name<T: Component>(&self, var: Variable<T>) -> Option<Cow<'static, str>> {
        let type_id = TypeId::of::<T>();
        self.vars
            .get(&type_id)?
            .any
            .get::<VarTable<T>>("any to be VarTable<T>")
            .data
            .get(var.index)?
            .name
            .clone()
    }

    fn variable_name_dyn(&self, var: VariableDyn) -> Option<Cow<'static, str>> {
        let dyn_caller = self.vars.get(&var.tid)?.dyn_caller;
        let mut name = None;
        dyn_caller(DynVarCommand::GetName(&mut |n| name = n), var, self);
        name
    }

    fn task_name<T: TaskData>(&self, task: Task<T>) -> Option<Cow<'static, str>> {
        let type_id = TypeId::of::<T>();
        self.tasks
            .get(&type_id)?
            .any
            .get::<TaskTable<T::Vars>>("any to be TaskTable<T>")
            .data
            .get(task.index)?
            .name
            .clone()
    }

    fn task_name_dyn(&self, task: TaskDyn) -> Option<Cow<'static, str>> {
        let dyn_caller = self.tasks.get(&task.tid)?.dyn_caller;
        let mut name = None;
        dyn_caller(DynTaskCommand::GetName(&mut |n| name = n), task, self);
        name
    }

    fn event_name<T: Component>(&self) -> Option<Cow<'static, str>> {
        let type_id = TypeId::of::<T>();
        self.events.get(&type_id)?.name.clone()
    }

    fn event_name_dyn(&self, event: TypeId) -> Option<Cow<'static, str>> {
        self.events.get(&event)?.name.clone()
    }

    fn effect_name<T: Component>(&self) -> Option<Cow<'static, str>> {
        let type_id = TypeId::of::<T>();
        self.effects.get(&type_id)?.name.clone()
    }

    fn effect_name_dyn(&self, effect: TypeId) -> Option<Cow<'static, str>> {
        self.effects.get(&effect)?.name.clone()
    }

    fn resource_name<T: Component>(&self) -> Option<Cow<'static, str>> {
        let type_id = TypeId::of::<T>();
        self.resources.get(&type_id)?.name.clone()
    }

    fn resource_name_dyn(&self, resource: TypeId) -> Option<Cow<'static, str>> {
        self.resources.get(&resource)?.name.clone()
    }
}

impl<T: Component> Variable<T> {
    fn erase(self) -> VariableDyn {
        VariableDyn {
            index: self.index,
            tid: TypeId::of::<T>(),
        }
    }
}

impl VariableDyn {
    fn downcast<T: Component>(self) -> Option<Variable<T>> {
        if self.tid == TypeId::of::<T>() {
            Some(Variable {
                index: self.index,
                _t: PhantomData::default(),
            })
        } else {
            None
        }
    }
}

pub struct EventValue<T>(PhantomData<T>);
#[allow(non_snake_case)]
pub fn Event<T: Component>() -> EventValue<T> {
    EventValue(PhantomData)
}

pub struct EffectValue<T>(PhantomData<T>);
#[allow(non_snake_case)]
pub fn Effect<T: Component>() -> EffectValue<T> {
    EffectValue(PhantomData)
}

pub struct ResourceValue<T: Component>(PhantomData<T>);
#[allow(non_snake_case)]
pub fn Resource<T: Component>() -> ResourceValue<T> {
    ResourceValue(PhantomData)
}

impl std::fmt::Display for TaskParameterDyn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskParameterDyn::Variable(v) => write!(f, "Var({})", v.index),
            TaskParameterDyn::Event(_) => write!(f, "Event()"),
            TaskParameterDyn::Effect(_) => write!(f, "Effect()"),
            TaskParameterDyn::Resource(_) => write!(f, "Res()"),
        }
    }
}

impl<T> Clone for TaskParameter<T> {
    fn clone(&self) -> Self {
        match self {
            TaskParameter::Variable(v) => TaskParameter::Variable(*v),
            TaskParameter::Event => TaskParameter::Event,
            TaskParameter::Effect => TaskParameter::Effect,
            TaskParameter::Resource => TaskParameter::Resource,
        }
    }
}

impl<T> Copy for TaskParameter<T> {}

impl<T: Component> TaskParameter<T> {
    fn get_mut_ptr(self, world: &World) -> Option<*mut T> {
        let type_id = TypeId::of::<T>();
        match self {
            TaskParameter::Variable(v) => world
                .vars
                .get(&type_id)?
                .any
                .get::<VarTable<T>>("any to be VarTable<T>")
                .data
                .get(v.index)
                .map(|cell| cell.t.get()),
            TaskParameter::Event => {
                let any = world.events.get(&type_id)?;
                let table = any.any.get::<EventTable<T>>("any to be EventTable<T>");
                if table.has_data.load(Ordering::Acquire) {
                    table.data.get().inner_ptr().some()
                } else {
                    None
                }
            }
            TaskParameter::Effect => world
                .effects
                .get(&type_id)?
                .any
                .get::<EffectTable<T>>("any to be EffectTable<T>")
                .data
                .get()
                .some(),
            TaskParameter::Resource => world
                .resources
                .get(&type_id)?
                .any
                .get::<ResourceTable<T>>("any to be ResourceTable<T>")
                .data
                .get()
                .some(),
        }
    }

    fn erase(self) -> TaskParameterDyn {
        match self {
            TaskParameter::Variable(v) => TaskParameterDyn::Variable(v.erase()),
            TaskParameter::Event => TaskParameterDyn::Event(TypeId::of::<T>()),
            TaskParameter::Effect => TaskParameterDyn::Effect(TypeId::of::<T>()),
            TaskParameter::Resource => TaskParameterDyn::Resource(TypeId::of::<T>()),
        }
    }
}

impl<T: Component> From<Variable<T>> for TaskParameter<T> {
    fn from(variable: Variable<T>) -> Self {
        Self::Variable(variable)
    }
}

impl<T: Component> From<EventValue<T>> for TaskParameter<T> {
    fn from(_: EventValue<T>) -> Self {
        Self::Event
    }
}

impl<T: Component> From<EffectValue<T>> for TaskParameter<T> {
    fn from(_: EffectValue<T>) -> Self {
        Self::Effect
    }
}

impl<T: Component> From<ResourceValue<T>> for TaskParameter<T> {
    fn from(_: ResourceValue<T>) -> Self {
        Self::Resource
    }
}

trait FnRetOverload<T> {
    fn param() -> TaskParameter<T>;
}

impl<T: Component> FnRetOverload<T> for EventValue<T> {
    fn param() -> TaskParameter<T> {
        TaskParameter::Event
    }
}

impl<T: Component> FnRetOverload<T> for EffectValue<T> {
    fn param() -> TaskParameter<T> {
        TaskParameter::Effect
    }
}

impl<T: Component> FnRetOverload<T> for ResourceValue<T> {
    fn param() -> TaskParameter<T> {
        TaskParameter::Resource
    }
}

impl<T: Component, R: FnRetOverload<T>, F: Fn() -> R> From<F> for TaskParameter<T> {
    fn from(_: F) -> Self {
        R::param()
    }
}

pub struct RefKindParameter<T> {
    param: TaskParameter<T>,
    ref_kind: RefKindVariant,
}

impl<T> Clone for RefKindParameter<T> {
    fn clone(&self) -> Self {
        Self {
            param: self.param,
            ref_kind: self.ref_kind,
        }
    }
}

impl TaskDyn {
    fn downcast<T: TaskData>(self) -> Option<Task<T>> {
        if self.tid == TypeId::of::<T>() {
            Some(Task {
                index: self.index,
                _t: PhantomData::default(),
            })
        } else {
            None
        }
    }
}

pub trait RefKind<T: Component> {
    /// casts the pointer to the right consumer type
    /// # Safety
    /// The pointer is generated with [`UnsafeCell::get`] and thus points to a valid value.
    /// Usage still needs to enforce the exclusivity / shared property of references, depending what type of reference the implementor creates.
    unsafe fn from_ref(t: *mut T) -> Self;
    fn variant() -> RefKindVariant;
    fn variant_for<V>(param: TaskParameter<V>) -> RefKindParameter<V> {
        RefKindParameter {
            param,
            ref_kind: Self::variant(),
        }
    }
}

pub struct Ref<'a, T: Component>(&'a T);
pub struct Mut<'a, T: Component>(&'a mut T);

impl<'a, T: Component> RefKind<T> for Ref<'a, T> {
    unsafe fn from_ref(t: *mut T) -> Self {
        Self(unsafe { &*t })
    }

    fn variant() -> RefKindVariant {
        RefKindVariant::Ref
    }
}

impl<'a, T: Component> RefKind<T> for Mut<'a, T> {
    unsafe fn from_ref(t: *mut T) -> Self {
        Self(unsafe { &mut *t })
    }

    fn variant() -> RefKindVariant {
        RefKindVariant::Mut
    }
}

impl<'a, T: Component> Deref for Ref<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}
impl<'a, T: Component> Deref for Mut<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}
impl<'a, T: Component> DerefMut for Mut<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
    }
}

impl<T: TaskData> Task<T> {
    pub fn erase(self) -> TaskDyn {
        TaskDyn {
            index: self.index,
            tid: TypeId::of::<T>(),
        }
    }
}

macro_rules! impl_call_from_world_dyn {
    ($fn_name:ident -> $task_new:ident,$task_new_named:ident -> $task_param:ident -> $($t:ident-$r:ident-$v:ident-$f:ident),+) => {

        pub struct $task_param<$( $t: Component ),+> {
            $($f: RefKindParameter<$t>),+
        }

        impl<$( $t: Component ),+> Clone for $task_param<$( $t ),+> {
            fn clone(&self) -> Self {
                Self {
                    $($f: self.$f.clone()),+
                }
            }
        }
        impl<$( $t: Component ),+> TaskDataInternal for $task_param<$( $t ),+> {
            fn register_vars(&self, register: &mut dyn VariableRegister) {
                $(
                    register.register_var(self.$f.ref_kind, self.$f.param.erase());
                )+
            }
        }
        impl<$( $t: Component ),+> TaskData for ($($t),+ ,) {
            type Vars = $task_param<$( $t ),+>;
        }

        #[allow(dead_code)]
        fn $fn_name<$($t: Component, $r: RefKind<$t>),+>(
            command: DynTaskCommand,
            task: TaskDyn,
            world: &World,
        ) {
            let Some(task) = task.downcast::<($($t),+,)>() else {eprintln!("unexpected typeid"); return};
            match command {
                DynTaskCommand::Call => {
                    let Some((f, a)) = world.task_fn(task) else {return};
                    $(
                        let $f = a.$f.param.get_mut_ptr(world);
                    )+
                    $(
                        let Some($f) = $f else {return;};
                    )+
                    let f: fn($($r),+) = unsafe { std::mem::transmute(f) };
                    (f)(
                        $(
                            unsafe { $r::from_ref($f) }
                        ),+
                    );
                },
                DynTaskCommand::ListVars(register) => {
                    world.register_vars(task, register);
                },
                DynTaskCommand::GetName(name) => {
                    name(world.task_name(task));
                }
            }
        }

        #[allow(dead_code)]
        impl World {
            pub fn $task_new<$($t: Component, $r: RefKind<$t>),+>(
                &self,
                f: fn($($r),+),
                $($v: impl Into<TaskParameter<$t>>,)+
            ) -> Task<($($t),+,)> {
                self.insert_task(
                    $task_param {
                        $($f: $r::variant_for($v.into()),)+
                    },
                    unsafe { std::mem::transmute(f) },
                    $fn_name::<$($t, $r),+>,
                    None
                )
            }

            pub fn $task_new_named<$($t: Component, $r: RefKind<$t>),+>(
                &self,
                name: impl Into<Cow<'static, str>>,
                f: fn($($r),+),
                $($v: impl Into<TaskParameter<$t>>,)+
            ) -> Task<($($t),+,)> {
                self.insert_task(
                    $task_param {
                        $($f: $r::variant_for($v.into()),)+
                    },
                    unsafe { std::mem::transmute(f) },
                    $fn_name::<$($t, $r),+>,
                    name.into().some(),
                )
            }
        }

        #[allow(dead_code)]
        impl Spawner {
            pub fn $task_new<$($t: Component, $r: RefKind<$t>),+>(
                &self,
                f: fn($($r),+),
                $($v: impl Into<TaskParameter<$t>>,)+
            ) -> Task<($($t),+,)> {
                self.world.insert_task(
                    $task_param {
                        $($f: $r::variant_for($v.into()),)+
                    },
                    unsafe { std::mem::transmute(f) },
                    $fn_name::<$($t, $r),+>,
                    None,
                )
            }

            pub fn $task_new_named<$($t: Component, $r: RefKind<$t>),+>(
                &self,
                name: impl Into<Cow<'static, str>>,
                f: fn($($r),+),
                $($v: impl Into<TaskParameter<$t>>,)+
            ) -> Task<($($t),+,)> {
                self.world.insert_task(
                    $task_param {
                        $($f: $r::variant_for($v.into()),)+
                    },
                    unsafe { std::mem::transmute(f) },
                    $fn_name::<$($t, $r),+>,
                    name.into().some(),
                )
            }
        }

        #[allow(dead_code)]
        impl WorldBuilder {
            pub fn $task_new<$($t: Component, $r: RefKind<$t>),+>(
                &self,
                f: fn($($r),+),
                $($v: impl Into<TaskParameter<$t>>,)+
            ) -> Task<($($t),+,)> {
                self.world.insert_task(
                    $task_param {
                        $($f: $r::variant_for($v.into()),)+
                    },
                    unsafe { std::mem::transmute(f) },
                    $fn_name::<$($t, $r),+>,
                    None,
                )
            }
            pub fn $task_new_named<$($t: Component, $r: RefKind<$t>),+>(
                &self,
                name: impl Into<Cow<'static, str>>,
                f: fn($($r),+),
                $($v: impl Into<TaskParameter<$t>>,)+
            ) -> Task<($($t),+,)> {
                self.world.insert_task(
                    $task_param {
                        $($f: $r::variant_for($v.into()),)+
                    },
                    unsafe { std::mem::transmute(f) },
                    $fn_name::<$($t, $r),+>,
                    name.into().some(),
                )
            }
        }
    };
}

impl_call_from_world_dyn!(call_from_world_dyn1 -> task1,task1_named -> TaskParameters1 -> T0-R0-v0-f0);
impl_call_from_world_dyn!(call_from_world_dyn2 -> task2,task2_named -> TaskParameters2 -> T0-R0-v0-f0, T1-R1-v1-f1);
impl_call_from_world_dyn!(call_from_world_dyn3 -> task3,task3_named -> TaskParameters3 -> T0-R0-v0-f0, T1-R1-v1-f1, T2-R2-v2-f2);
