/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::{
    any::{Any, TypeId},
    cell::UnsafeCell,
    collections::HashMap,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

pub mod spawner;
mod stable_vec;

use spawner::Spawner;
use stable_vec::SVec;

pub trait Component: Send + 'static {}
impl<T: Send + 'static> Component for T {}

struct TrackedVariable<T> {
    variable: UnsafeCell<T>,
    dependents: Vec<TaskDyn>,
}

impl<T> TrackedVariable<T> {
    fn new(t: T) -> Self {
        Self {
            variable: UnsafeCell::new(t),
            dependents: Vec::new(),
        }
    }
}

struct VarTable<T> {
    data: SVec<UnsafeCell<T>>,
    gen: SVec<(usize, bool)>,
}

struct EventVarTable<T> {
    data: SVec<TrackedVariable<T>>,
    gen: SVec<(usize, bool)>,
}

struct TaskTable<T> {
    data: SVec<T>,
    gen: SVec<usize>,
}

struct ResourceTable<T> {
    data: UnsafeCell<T>,
    in_use: bool,
}

impl<T> VarTable<T> {
    fn new() -> Self {
        Self {
            data: SVec::new(),
            gen: SVec::new(),
        }
    }
}

impl<T> EventVarTable<T> {
    fn new() -> Self {
        Self {
            data: SVec::new(),
            gen: SVec::new(),
        }
    }
}

impl<T> TaskTable<T> {
    fn new() -> Self {
        Self {
            data: SVec::new(),
            gen: SVec::new(),
        }
    }
}

impl<T> ResourceTable<T> {
    fn new(data: T) -> Self {
        Self {
            data: UnsafeCell::new(data),
            in_use: false,
        }
    }
}

pub struct World {
    vars: HashMap<TypeId, Box<dyn Any + Send>>,
    event_vars: HashMap<TypeId, Box<dyn Any + Send>>,
    tasks: HashMap<TypeId, Box<dyn Any + Send>>,
    resources: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl World {
    pub fn new() -> Self {
        World {
            vars: HashMap::new(),
            event_vars: HashMap::new(),
            tasks: HashMap::new(),
            resources: HashMap::new(),
        }
    }

    fn insert_var<T: Component>(&mut self, t: T) -> Variable<T> {
        let table = self
            .vars
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(VarTable::<T>::new()) as Box<dyn Any + Send>)
            .as_mut()
            .downcast_mut::<VarTable<T>>()
            .expect("Any to be a VarTable<T>");
        table.gen.push((0, false));
        Variable {
            index: table.data.push(UnsafeCell::new(t)),
            generation: 0,
            _t: PhantomData::default(),
        }
    }

    fn insert_event_var<T: Component>(&mut self, t: T) -> EventVariable<T> {
        let table = self
            .event_vars
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(EventVarTable::<T>::new()) as Box<dyn Any + Send>)
            .as_mut()
            .downcast_mut::<EventVarTable<T>>()
            .expect("Any to be a EventVarTable<T>");
        table.gen.push((0, false));
        EventVariable {
            index: table.data.push(TrackedVariable::new(t)),
            generation: 0,
            _t: PhantomData::default(),
        }
    }

    fn insert_task<T: Component>(&mut self, t: T) -> TaskData<T> {
        let table = self
            .tasks
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(TaskTable::<T>::new()) as Box<dyn Any + Send>)
            .as_mut()
            .downcast_mut::<TaskTable<T>>()
            .expect("Any to be a TaskTable<T>");
        table.gen.push(0);
        TaskData {
            index: table.data.push(t),
            generation: 0,
            _t: PhantomData::default(),
        }
    }

    pub fn insert_or_replace_resource<T: Component>(&mut self, mut t: T) -> Option<T> {
        match self.resources.entry(TypeId::of::<T>()) {
            std::collections::hash_map::Entry::Occupied(o) => {
                std::mem::swap(
                    o.into_mut()
                        .downcast_mut::<ResourceTable<T>>()
                        .expect("Any to be a ResourceTable<T>")
                        .data
                        .get_mut(),
                    &mut t,
                );
                Some(t)
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(Box::new(ResourceTable::new(t)) as Box<dyn Any + Send>);
                None
            }
        }
    }

    fn register_vars<T: TaskParameters>(
        &mut self,
        task_vars: TaskData<T>,
        register: &mut dyn VariableRegister,
    ) {
        let Some(task_vars) = task_vars.get(self) else {return};
        task_vars.register_vars(register);
    }

    pub fn start(mut self, f: TaskDyn) {
        self.insert_or_replace_resource(Spawner::new());
        f.call_from_world(&mut self);
    }
}

pub struct Variable<T> {
    index: usize,
    generation: usize,
    _t: PhantomData<T>,
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub struct VariableDyn {
    index: usize,
    generation: usize,
    tid: TypeId,
}

impl<T> Clone for Variable<T> {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            generation: self.generation,
            _t: PhantomData,
        }
    }
}

impl<T> Copy for Variable<T> {}

impl<T: Component> Variable<T> {
    pub fn new(t: T, world: &mut World) -> Self {
        world.insert_var(t)
    }

    pub fn set(self, t: T, world: &mut World) {
        let Some(table) = world
            .vars
            .get_mut(&TypeId::of::<T>()) else {return};
        let table = table
            .downcast_mut::<VarTable<T>>()
            .expect("any to be VarTable<T>");
        let Some(&(gen, in_use)) = table.gen.get(self.index) else {return};
        if gen != self.generation || in_use {
            return;
        }
        let Some(var) = table.data.get_mut(self.index) else {return};
        *var.get_mut() = t;
    }

    fn erase(self) -> VariableDyn {
        VariableDyn {
            index: self.index,
            generation: self.generation,
            tid: TypeId::of::<T>(),
        }
    }
}

pub struct EventVariable<T> {
    index: usize,
    generation: usize,
    _t: PhantomData<T>,
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub struct EventVariableDyn {
    index: usize,
    generation: usize,
    tid: TypeId,
}

impl<T> Clone for EventVariable<T> {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            generation: self.generation,
            _t: self._t,
        }
    }
}

impl<T> Copy for EventVariable<T> {}

impl<T: Component> EventVariable<T> {
    fn new(t: T, world: &mut World) -> Self {
        world.insert_event_var(t)
    }

    fn erase(self) -> EventVariableDyn {
        EventVariableDyn {
            index: self.index,
            generation: self.generation,
            tid: TypeId::of::<T>(),
        }
    }
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
    EventVar(EventVariable<T>),
    Resource(),
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub enum TaskParameterDyn {
    Variable(VariableDyn),
    EventVar(EventVariableDyn),
    Resource(TypeId),
}

impl<T> Clone for TaskParameter<T> {
    fn clone(&self) -> Self {
        match self {
            TaskParameter::Variable(v) => TaskParameter::Variable(*v),
            TaskParameter::EventVar(e) => TaskParameter::EventVar(*e),
            TaskParameter::Resource() => TaskParameter::Resource(),
        }
    }
}

impl<T> Copy for TaskParameter<T> {}

impl<T: Component> TaskParameter<T> {
    fn get_mut_ptr(self, world: &mut World) -> Option<*mut T> {
        match self {
            TaskParameter::Variable(v) => {
                let table = world
                    .vars
                    .get_mut(&TypeId::of::<T>())?
                    .downcast_mut::<VarTable<T>>()
                    .expect("any to be VarTable<T>");
                let (gen, in_use) = table.gen.get_mut(v.index)?;
                if *gen != v.generation {
                    return None;
                }
                if *in_use {
                    return None;
                }
                *in_use = true;
                table.data.get(v.index).map(UnsafeCell::get)
            }
            TaskParameter::EventVar(e) => {
                let table = world
                    .event_vars
                    .get_mut(&TypeId::of::<T>())?
                    .downcast_mut::<EventVarTable<T>>()
                    .expect("any to be EventVarTable<T>");
                let (gen, in_use) = table.gen.get_mut(e.index)?;
                if *gen != e.generation {
                    return None;
                }
                if *in_use {
                    return None;
                }
                *in_use = true;
                table.data.get(e.index).map(|tv| tv.variable.get())
            }
            TaskParameter::Resource() => {
                let res = world
                    .resources
                    .get_mut(&TypeId::of::<T>())?
                    .downcast_mut::<ResourceTable<T>>()
                    .expect("any to be ResourceTable<T>");
                if res.in_use {
                    return None;
                }
                res.in_use = true;
                Some(res.data.get())
            }
        }
    }

    fn reset_mut_ptr(self, world: &mut World) {
        match self {
            TaskParameter::Variable(v) => {
                let Some(table) = world
                    .vars
                    .get_mut(&TypeId::of::<T>()) else {return};
                let table = table
                    .downcast_mut::<VarTable<T>>()
                    .expect("any to be VarTable<T>");
                let Some((gen, in_use)) = table.gen.get_mut(v.index) else {return};
                if *gen != v.generation {
                    return;
                }
                *in_use = false;
            }
            TaskParameter::EventVar(e) => {
                let Some(table) = world
                    .vars
                    .get_mut(&TypeId::of::<T>()) else {return};
                let table = table
                    .downcast_mut::<EventVarTable<T>>()
                    .expect("any to be EventVarTable<T>");
                let Some((gen, in_use)) = table.gen.get_mut(e.index) else {return};
                if *gen != e.generation {
                    return;
                }
                *in_use = false;
            }

            TaskParameter::Resource() => {
                let Some(res) = world
                    .resources
                    .get_mut(&TypeId::of::<T>()) else {return};
                res.downcast_mut::<ResourceTable<T>>()
                    .expect("any to be ResourceTable<T>")
                    .in_use = false;
            }
        }
    }

    fn erase(self) -> TaskParameterDyn {
        match self {
            TaskParameter::Variable(v) => TaskParameterDyn::Variable(v.erase()),
            TaskParameter::EventVar(e) => TaskParameterDyn::EventVar(e.erase()),
            TaskParameter::Resource() => TaskParameterDyn::Resource(TypeId::of::<T>()),
        }
    }
}

impl<T: Component> From<Variable<T>> for TaskParameter<T> {
    fn from(variable: Variable<T>) -> Self {
        Self::Variable(variable)
    }
}

impl<T: Component> From<EventVariable<T>> for TaskParameter<T> {
    fn from(event_var: EventVariable<T>) -> Self {
        Self::EventVar(event_var)
    }
}

impl<T: Component> From<ResourceValue<T>> for TaskParameter<T> {
    fn from(_: ResourceValue<T>) -> Self {
        Self::Resource()
    }
}

impl<T: Component, F: Fn() -> ResourceValue<T>> From<F> for TaskParameter<T> {
    fn from(_: F) -> Self {
        Self::Resource()
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
    generation: usize,
    _t: PhantomData<T>,
}

#[derive(Clone, Copy)]
struct TaskDataDyn {
    index: usize,
    generation: usize,
    tid: TypeId,
}

impl TaskDataDyn {
    fn downcast<T: Component>(self) -> Option<TaskData<T>> {
        if self.tid == TypeId::of::<T>() {
            Some(TaskData {
                index: self.index,
                generation: self.generation,
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
            generation: self.generation,
            _t: PhantomData,
        }
    }
}

impl<T> Copy for TaskData<T> {}

impl<T: Component> TaskData<T> {
    fn erase(self) -> TaskDataDyn {
        TaskDataDyn {
            index: self.index,
            generation: self.generation,
            tid: TypeId::of::<T>(),
        }
    }
}

impl<T: Component + Clone> TaskData<T> {
    fn get(self, world: &mut World) -> Option<T> {
        let table = world
            .tasks
            .get(&TypeId::of::<T>())?
            .downcast_ref::<TaskTable<T>>()
            .expect("any to be TaskTable<T>");
        let gen = *table.gen.get(self.index)?;
        if gen != self.generation {
            return None;
        }
        table.data.get(self.index).cloned()
    }
}

pub trait RefKind<T: Component> {
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
    fn call_from_world(self, a: TaskData<Vars::Vars>, world: &mut World);
}

impl Call<()> for fn() {
    fn call_from_world(self, _a: TaskData<<() as TaskParam>::Vars>, _world: &mut World) {
        (self)();
    }
}

enum DynCommand<'a> {
    Call(unsafe fn()),
    ListVars(&'a mut dyn VariableRegister),
}

pub struct Task<Vars: TaskParam> {
    f: unsafe fn(),
    dyn_caller: unsafe fn(DynCommand, TaskDataDyn, &mut World),
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

#[derive(Clone, Copy)]
pub struct TaskDyn {
    f: unsafe fn(),
    dyn_caller: unsafe fn(DynCommand, TaskDataDyn, &mut World),
    v: TaskDataDyn,
}

impl<T: TaskParam> Task<T> {
    pub fn call_from_world(self, world: &mut World) {
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

    pub fn register_vars(&self, register: &mut dyn VariableRegister, world: &mut World) {
        unsafe {
            (self.dyn_caller)(DynCommand::ListVars(register), self.v.erase(), world);
        }
    }
}

impl TaskDyn {
    pub fn call_from_world(self, world: &mut World) {
        unsafe {
            (self.dyn_caller)(DynCommand::Call(self.f), self.v, world);
        }
    }
}

unsafe fn call_from_world_dyn0(command: DynCommand, variables: TaskDataDyn, world: &mut World) {
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
    pub fn new0(f: fn(), world: &mut World) -> Self {
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
            fn call_from_world(self, a: TaskData<<($($t),+,) as TaskParam>::Vars>, world: &mut World) {
                let Some(a) = a.get(world) else {return};
                $(
                    let $f = a.$f.param.get_mut_ptr(world);
                )+
                let inner = |world: &mut World| {
                    $(
                        let Some($f) = $f else {return true};
                    )+
                    (self)(
                        $(
                            unsafe { $r::from_ref($f) }
                        ),+
                    );
                    $(
                        a.$f.param.reset_mut_ptr(world);
                    )+
                    return false;
                };
                if inner(world) {
                    $(
                        a.$f.param.reset_mut_ptr(world);
                    )+
                }
            }
        }

        #[allow(dead_code)]
        unsafe fn $fn_name<$($t: Component, $r: RefKind<$t>),+>(
            command: DynCommand,
            variables: TaskDataDyn,
            world: &mut World,
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
        impl<$($t: Component),+> Task<($($t),+,)> {
            pub fn $task_new<$($r: RefKind<$t>),+>(
                f: fn($($r),+),
                $($v: impl Into<TaskParameter<$t>>,)+
                world: &mut World,
            ) -> Self {
                Self {
                    f: unsafe { std::mem::transmute(f) },
                    dyn_caller: $fn_name::<$($t, $r),+>,
                    v: world.insert_task($task_param {
                        $($f: $r::variant_for($v.into()),)+
                    }),
                }
            }
        }
    };
}

impl_call_from_world_dyn!(call_from_world_dyn1 -> new1 -> TaskParameters1 -> T0-R0-v0-f0);
impl_call_from_world_dyn!(call_from_world_dyn2 -> new2 -> TaskParameters2 -> T0-R0-v0-f0, T1-R1-v1-f1);
impl_call_from_world_dyn!(call_from_world_dyn3 -> new3 -> TaskParameters3 -> T0-R0-v0-f0, T1-R1-v1-f1, T2-R2-v2-f2);
