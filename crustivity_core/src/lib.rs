/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#![feature(strict_provenance)]

use std::{
    any::{Any, TypeId},
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
pub use crate::world::World;
use constraints::{Constraint, ConstraintSystem, EffectPath};
use dashmap::DashMap;
use parking_lot::Mutex;
use sharded_slab::Slab;
use world::{ActivationEntry, AnyType};

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

pub trait Component: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Component for T {}

struct VarTable<T> {
    data: Slab<UnsafeCell<T>>,
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

struct TaskTable<T> {
    data: Slab<UnsafeCell<T>>,
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

impl AnyType {
    fn variable<T: Component>() -> Self {
        Self {
            any: Box::new(VarTable::<T>::new()) as Box<dyn Any + Send + Sync>,
        }
    }

    fn event<T: Component>() -> Self {
        Self {
            any: Box::new(EventTable::<T>::new()) as Box<dyn Any + Send + Sync>,
        }
    }

    fn effect<T: Component + Default>() -> Self {
        Self {
            any: Box::new(EffectTable::<T>::new()) as Box<dyn Any + Send + Sync>,
        }
    }

    fn resource<T: Component>(t: T) -> Self {
        Self {
            any: Box::new(ResourceTable::new(t)) as Box<dyn Any + Send + Sync>,
        }
    }

    fn task<T: Component>() -> Self {
        Self {
            any: Box::new(TaskTable::<T>::new()) as Box<dyn Any + Send + Sync>,
        }
    }

    fn get<TTable: Send + 'static>(&self, err_msg: &'static str) -> &TTable {
        self.any.downcast_ref::<TTable>().expect(err_msg)
    }
}

pub struct WorldBuilder {
    world: World,
}

impl WorldBuilder {
    pub fn new() -> Self {
        Self {
            world: World::new(),
        }
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

    pub fn variable<T: Component>(&self, t: T) -> Variable<T> {
        self.world.variable(t)
    }

    pub fn resource<T: Component>(&self, t: T) -> Result<(), T> {
        self.world.resource(t)
    }

    pub fn register_effect<T: Component + Default>(&self) {
        self.world.register_effect::<T>()
    }

    pub fn register_event<T: Component>(&self, t: T) -> Result<(), T> {
        self.world.register_event(t)
    }

    pub fn constraint<T: TaskParam>(&self, task: Task<T>) -> Constraint {
        self.world.constraint(task)
    }
}

impl World {
    fn new() -> Self {
        Self {
            vars: DashMap::new(),
            events: DashMap::new(),
            effects: DashMap::new(),
            tasks: DashMap::new(),
            resources: DashMap::new(),

            activations: Mutex::new(VecDeque::new()),
            insertion_lock: Mutex::new(()),
        }
    }

    pub fn variable<T: Component>(&self, t: T) -> Variable<T> {
        let type_id = TypeId::of::<T>();
        let any = if let Some(any) = self.vars.get(&type_id) {
            any
        } else {
            let insertion_guard = self.insertion_lock.lock();
            if let Some(any) = self.vars.get(&type_id) {
                any
            } else {
                let previous = self.vars.insert(type_id, AnyType::variable::<T>());
                drop(insertion_guard);
                if previous.is_some() {
                    log::warn!("Insertions of new tasks, that were already present, should not happen and may result in undefined behaviour.");
                }
                self.vars.get(&type_id).unwrap()
            }
        };
        let table = any.get::<VarTable<T>>("any to be a VarTable<T>");
        Variable {
            index: table
                .data
                .insert(UnsafeCell::new(t))
                .expect("out of memory"),
            _t: PhantomData::default(),
        }
    }

    fn insert_task<T: Component>(&self, t: T) -> TaskData<T> {
        let type_id = TypeId::of::<T>();
        let any = if let Some(any) = self.tasks.get(&type_id) {
            any
        } else {
            let insertion_guard = self.insertion_lock.lock();
            if let Some(any) = self.tasks.get(&type_id) {
                any
            } else {
                let previous = self.tasks.insert(type_id, AnyType::task::<T>());
                drop(insertion_guard);
                if previous.is_some() {
                    log::warn!("Insertions of new tasks, that were already present, should not happen and may result in undefined behaviour.");
                }
                self.tasks.get(&type_id).unwrap()
            }
        };
        let table = any.get::<TaskTable<T>>("any to be a TaskTable<T>");
        TaskData {
            index: table
                .data
                .insert(UnsafeCell::new(t))
                .expect("out of memory"),
            _t: PhantomData::default(),
        }
    }

    pub fn resource<T: Component>(&self, t: T) -> Result<(), T> {
        let type_id = TypeId::of::<T>();
        if self.resources.contains_key(&type_id) {
            Err(t)
        } else {
            let insertion_guard = self.insertion_lock.lock();
            if self.resources.contains_key(&type_id) {
                Err(t)
            } else {
                let previous = self.resources.insert(type_id, AnyType::resource(t));
                drop(insertion_guard);
                if previous.is_some() {
                    log::warn!("Insertions of new resources, that were already present, should not happen and may result in undefined behaviour.");
                }
                Ok(())
            }
        }
    }

    fn register_vars<T: TaskParameters>(
        &self,
        task_vars: TaskData<T>,
        register: &mut dyn VariableRegister,
    ) {
        let Some(task_vars) = task_vars.get(self) else {return};
        task_vars.register_vars(register);
    }

    pub fn register_effect<T: Component + Default>(&self) {
        let type_id = TypeId::of::<T>();
        if !self.effects.contains_key(&type_id) {
            let insertion_guard = self.insertion_lock.lock();
            if !self.effects.contains_key(&type_id) {
                let previous = self
                    .effects
                    .insert(type_id, (AnyType::effect::<T>(), World::finish_effect::<T>));
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
        let effect = any.0.get::<EffectTable<T>>("any to be EffectTable<T>");
        let mut activations = effect.acivations.lock();
        std::mem::take(&mut *activations)
    }

    pub fn register_event<T: Component>(&self, t: T) -> Result<(), T> {
        let type_id = TypeId::of::<T>();
        if self.events.contains_key(&type_id) {
            Err(t)
        } else {
            let insertion_guard = self.insertion_lock.lock();
            if self.events.contains_key(&type_id) {
                Err(t)
            } else {
                let previous = self.events.insert(type_id, AnyType::event::<T>());
                let any = self.events.get(&type_id).unwrap();
                let event = any.get::<EventTable<T>>("any to be an EventTable<T>");
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
                let previous = self.events.insert(type_id, AnyType::event::<T>());
                drop(insertion_guard);
                if previous.is_some() {
                    log::warn!("Insertions of new events, that were already present, should not happen and may result in undefined behaviour.");
                }
                self.events.get(&type_id).unwrap()
            }
        };
        let event_table = any.get::<EventTable<T>>("any to be an EventTable<T>");
        event_table.activations.lock().push_back(t);

        self.activations
            .lock()
            .push_back(ActivationEntry(World::write_activation_and_emit::<T>));
    }

    fn write_activation_and_emit<T: Component>(&self) {
        let type_id = TypeId::of::<T>();
        let Some(any) = self.events.get(&type_id) else {return;};
        let event = any.get::<EventTable<T>>("any to be EventTable<T>");
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
        let effect = any.0.get::<EffectTable<T>>("any to be EffectTable<T>");
        let data_ptr = effect.data.get();
        // TODO: lock
        let data_ref = unsafe { &mut *data_ptr };
        let t = std::mem::take(data_ref);
        effect.acivations.lock().push(t);
    }

    fn execute_effect_path(&self, path: EffectPath) {
        for entry in path.tasks {
            entry.call_from_world(self);
        }
        for finisher in path
            .effects
            .iter()
            .flat_map(|tid| self.effects.get(tid).into_iter())
            .map(|any| any.1)
        {
            finisher(self);
        }
    }

    pub fn constraint<T: TaskParam>(&self, task: Task<T>) -> Constraint {
        Constraint::new(task, self)
    }
}

pub struct Variable<T> {
    index: usize,
    _t: PhantomData<T>,
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub struct VariableDyn {
    index: usize,
    tid: TypeId,
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

impl<T: Component> Variable<T> {
    fn erase(self) -> VariableDyn {
        VariableDyn {
            index: self.index,
            tid: TypeId::of::<T>(),
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RefKindVariant {
    Ref,
    Mut,
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
                .get::<VarTable<T>>("any to be VarTable<T>")
                .data
                .get(v.index)
                .map(|cell| cell.get()),
            TaskParameter::Event => {
                let any = world.events.get(&type_id)?;
                let table = any.get::<EventTable<T>>("any to be EventTable<T>");
                if table.has_data.load(Ordering::Acquire) {
                    table.data.get().inner_ptr().some()
                } else {
                    None
                }
            }
            TaskParameter::Effect => world
                .effects
                .get(&type_id)?
                .0
                .get::<EffectTable<T>>("any to be EffectTable<T>")
                .data
                .get()
                .some(),
            TaskParameter::Resource => world
                .resources
                .get(&type_id)?
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

struct TaskData<T> {
    index: usize,
    _t: PhantomData<T>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TaskDataDyn {
    index: usize,
    tid: TypeId,
}

impl TaskDataDyn {
    fn downcast<T: Component>(self) -> Option<TaskData<T>> {
        if self.tid == TypeId::of::<T>() {
            Some(TaskData {
                index: self.index,
                _t: PhantomData::default(),
            })
        } else {
            None
        }
    }
}

impl<T> Clone for TaskData<T> {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            _t: PhantomData,
        }
    }
}

impl<T> Copy for TaskData<T> {}

impl<T: Component> TaskData<T> {
    fn erase(self) -> TaskDataDyn {
        TaskDataDyn {
            index: self.index,
            tid: TypeId::of::<T>(),
        }
    }
}

impl<T: Component + Clone> TaskData<T> {
    fn get(self, world: &World) -> Option<T> {
        world
            .tasks
            .get(&TypeId::of::<T>())?
            .get::<TaskTable<T>>("any to be TaskTable<T>")
            .data
            .get(self.index)
            .map(|cell| unsafe { &*cell.get() }.clone())
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

pub trait VariableRegister {
    fn register_var(&mut self, ref_kind: RefKindVariant, var: TaskParameterDyn);
}

pub trait TaskParameters: Clone + Component {
    fn register_vars(&self, register: &mut dyn VariableRegister);
}

pub trait TaskParam {
    type Vars: TaskParameters;
}

#[derive(Clone)]
pub struct TaskParameter0 {}
impl TaskParameters for TaskParameter0 {
    fn register_vars(&self, _registrer: &mut dyn VariableRegister) {}
}

impl TaskParam for () {
    type Vars = TaskParameter0;
}

trait Call<Vars: TaskParam> {
    fn call_from_world(self, a: TaskData<Vars::Vars>, world: &World);
}

impl Call<()> for fn() {
    fn call_from_world(self, _a: TaskData<<() as TaskParam>::Vars>, _world: &World) {
        (self)();
    }
}

enum DynCommand<'a> {
    Call(unsafe fn()),
    ListVars(&'a mut dyn VariableRegister),
}

pub struct Task<Vars: TaskParam> {
    f: unsafe fn(),
    dyn_caller: unsafe fn(DynCommand, TaskDataDyn, &World),
    v: TaskData<Vars::Vars>,
}

impl<Vars: TaskParam> Clone for Task<Vars> {
    fn clone(&self) -> Self {
        Self {
            f: self.f,
            dyn_caller: self.dyn_caller,
            v: self.v,
        }
    }
}

impl<Vars: TaskParam> Copy for Task<Vars> {}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskDyn {
    f: unsafe fn(),
    dyn_caller: unsafe fn(DynCommand, TaskDataDyn, &World),
    v: TaskDataDyn,
}

impl<T: TaskParam> Task<T> {
    pub fn call_from_world(self, world: &World) {
        unsafe {
            (self.dyn_caller)(DynCommand::Call(self.f), self.v.erase(), world);
        }
    }

    pub fn erase(self) -> TaskDyn {
        TaskDyn {
            f: self.f,
            dyn_caller: self.dyn_caller,
            v: self.v.erase(),
        }
    }

    pub fn register_vars(&self, register: &mut dyn VariableRegister, world: &World) {
        unsafe {
            (self.dyn_caller)(DynCommand::ListVars(register), self.v.erase(), world);
        }
    }
}

impl TaskDyn {
    pub fn call_from_world(self, world: &World) {
        unsafe {
            (self.dyn_caller)(DynCommand::Call(self.f), self.v, world);
        }
    }
}

unsafe fn call_from_world_dyn0(command: DynCommand, variables: TaskDataDyn, world: &World) {
    let Some(exact_variables) = variables.downcast::<<() as TaskParam>::Vars>() else {eprintln!("unexpected typeid"); return};
    match command {
        DynCommand::Call(f) => {
            let exact_f: fn() = unsafe { std::mem::transmute(f) };
            exact_f.call_from_world(exact_variables, world);
        }
        DynCommand::ListVars(register) => {
            world.register_vars(exact_variables, register);
        }
    }
}

impl Task<()> {
    pub fn new0(f: fn(), world: &World) -> Self {
        Self {
            f: unsafe { std::mem::transmute(f) },
            dyn_caller: call_from_world_dyn0,
            v: world.insert_task(TaskParameter0 {}),
        }
    }
}

macro_rules! impl_call_from_world_dyn {
    ($fn_name:ident -> $task_new:ident -> $task_param:ident -> $($t:ident-$r:ident-$v:ident-$f:ident),+) => {

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
        impl<$( $t: Component ),+> TaskParameters for $task_param<$( $t ),+> {
            fn register_vars(&self, register: &mut dyn VariableRegister) {
                $(
                    register.register_var(self.$f.ref_kind, self.$f.param.erase());
                )+
            }
        }
        impl<$( $t: Component ),+> TaskParam for ($($t),+ ,) {
            type Vars = $task_param<$( $t ),+>;
        }

        impl<$($t: Component, $r: RefKind<$t>),+> Call<($($t),+,)> for fn($($r),+) {
            fn call_from_world(self, a: TaskData<<($($t),+,) as TaskParam>::Vars>, world: &World
    ) {
                let Some(a) = a.get(world) else {return};
                $(
                    let $f = a.$f.param.get_mut_ptr(world);
                )+
                $(
                    let Some($f) = $f else {return;};
                )+
                (self)(
                    $(
                        unsafe { $r::from_ref($f) }
                    ),+
                );
            }
        }

        #[allow(dead_code)]
        unsafe fn $fn_name<$($t: Component, $r: RefKind<$t>),+>(
            command: DynCommand,
            variables: TaskDataDyn,
            world: &World
    ,
        ) {
            let Some(exact_variables) = variables.downcast::<<($($t),+,) as TaskParam>::Vars>() else {eprintln!("unexpected typeid"); return};
            match command {
                DynCommand::Call(f) => {
                    let exact_f: fn($($r),+) = unsafe { std::mem::transmute(f) };
                    exact_f.call_from_world(exact_variables, world);
                },
                DynCommand::ListVars(register) => {
                    world.register_vars(exact_variables, register);
                },
            }
        }

        #[allow(dead_code)]
        impl World {
            pub fn $task_new<$($t: Component, $r: RefKind<$t>),+>(
                &self,
                f: fn($($r),+),
                $($v: impl Into<TaskParameter<$t>>,)+
            ) -> Task<($($t),+,)> {
                Task {
                    f: unsafe { std::mem::transmute(f) },
                    dyn_caller: $fn_name::<$($t, $r),+>,
                    v: self.insert_task($task_param {
                        $($f: $r::variant_for($v.into()),)+
                    }),
                }
            }
        }

        #[allow(dead_code)]
        impl Spawner {
            pub fn $task_new<$($t: Component, $r: RefKind<$t>),+>(
                &self,
                f: fn($($r),+),
                $($v: impl Into<TaskParameter<$t>>,)+
            ) -> Task<($($t),+,)> {
                Task {
                    f: unsafe { std::mem::transmute(f) },
                    dyn_caller: $fn_name::<$($t, $r),+>,
                    v: self.world.insert_task($task_param {
                        $($f: $r::variant_for($v.into()),)+
                    }),
                }
            }
        }

        #[allow(dead_code)]
        impl WorldBuilder {
            pub fn $task_new<$($t: Component, $r: RefKind<$t>),+>(
                &self,
                f: fn($($r),+),
                $($v: impl Into<TaskParameter<$t>>,)+
            ) -> Task<($($t),+,)> {
                Task {
                    f: unsafe { std::mem::transmute(f) },
                    dyn_caller: $fn_name::<$($t, $r),+>,
                    v: self.world.insert_task($task_param {
                        $($f: $r::variant_for($v.into()),)+
                    }),
                }
            }
        }
    };
}

impl_call_from_world_dyn!(call_from_world_dyn1 -> task1 -> TaskParameters1 -> T0-R0-v0-f0);
impl_call_from_world_dyn!(call_from_world_dyn2 -> task2 -> TaskParameters2 -> T0-R0-v0-f0, T1-R1-v1-f1);
impl_call_from_world_dyn!(call_from_world_dyn3 -> task3 -> TaskParameters3 -> T0-R0-v0-f0, T1-R1-v1-f1, T2-R2-v2-f2);
