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

mod stable_vec;

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
    data: SVec<TrackedVariable<T>>,
    gen: SVec<(usize, bool)>,
}

struct TaskTable<T> {
    data: SVec<T>,
    gen: SVec<usize>,
}

impl<T> VarTable<T> {
    fn new() -> Self {
        VarTable {
            data: SVec::new(),
            gen: SVec::new(),
        }
    }
}

impl<T> TaskTable<T> {
    fn new() -> Self {
        TaskTable {
            data: SVec::new(),
            gen: SVec::new(),
        }
    }
}

pub struct World {
    vars: HashMap<TypeId, Box<dyn Any + Send>>,
    tasks: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl World {
    pub fn new() -> Self {
        World {
            vars: HashMap::new(),
            tasks: HashMap::new(),
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
            index: table.data.push(TrackedVariable::new(t)),
            generation: 0,
            _t: PhantomData::default(),
        }
    }

    fn register_task_for_var<T: Component>(&mut self, task: TaskDyn, v: Variable<T>) {
        let Some(table) = self.vars.get_mut(&TypeId::of::<T>()) else {return};
        let Some(table) = table.as_mut().downcast_mut::<VarTable<T>>() else {return};
        let Some(var) = table.data.get_mut(v.index) else {return};
        var.dependents.push(task);
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

    pub fn effect<T: TaskParam>(&mut self, task: Task<T>) {
        let Some(task_data) = task.v.get(self) else {return};
        task_data.register_vars(task.erase(), self);
    }
}

pub struct Variable<T> {
    index: usize,
    generation: usize,
    _t: PhantomData<T>,
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

    fn get_mut(self, world: &mut World) -> Option<(*mut T, Vec<TaskDyn>)> {
        let table = world
            .vars
            .get_mut(&TypeId::of::<T>())?
            .downcast_mut::<VarTable<T>>()
            .expect("any to be VarTable<T>");
        let (gen, in_use) = table.gen.get_mut(self.index)?;
        if *gen != self.generation {
            return None;
        }
        if *in_use {
            return None;
        }
        *in_use = true;
        let var = table.data.get(self.index)?;
        Some((var.variable.get(), var.dependents.clone()))
    }

    fn reset_mut(self, world: &mut World) {
        let Some(table) = world
            .vars
            .get_mut(&TypeId::of::<T>()) else {return};
        let table = table
            .downcast_mut::<VarTable<T>>()
            .expect("any to be VarTable<T>");
        let Some((gen, in_use)) = table.gen.get_mut(self.index) else {return};
        if *gen != self.generation {
            return;
        }
        *in_use = false;
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
        *var.variable.get_mut() = t;
        for d in var.dependents.clone() {
            d.call_from_world(world);
        }
    }

    pub fn print_meta_data(self, world: &mut World) {
        let Some(table) = world
            .vars
            .get(&TypeId::of::<T>()) else {

            println!("Variable<{}> {{\n\t(no table)\n}}", std::any::type_name::<T>());
            return
        };
        let table = table
            .downcast_ref::<VarTable<T>>()
            .expect("any to be VarTable<T>");
        let Some(&(gen, in_use)) = table.gen.get(self.index) else {
            println!(
                "Variable<{}> {{\n\tindex: {} (out-of-bounds),\n\tgeneration: {} (out-of-bounds),\n}}", 
                std::any::type_name::<T>(),
                self.index,
                self.generation,
            ); 
            return;
        };
        if gen == self.generation {
            println!(
                "Variable<{}> {{\n\tindex: {},\n\tgeneration: {},\n\tin_use: {}\n\tdependents_count: {}\n}}",
                std::any::type_name::<T>(),
                self.index,
                self.generation,
                in_use,
                table.data.get(self.index).unwrap().dependents.len(),
            );
        } else {
            println!(
                "Variable<{}> {{\n\tindex: {},\n\tgeneration: {} (!={})\n}}",
                std::any::type_name::<T>(),
                self.index,
                self.generation,
                gen
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RefKindVariant {
    Ref,
    Mut,
}

pub struct RefKindVariable<T> {
    var: Variable<T>,
    ref_kind: RefKindVariant,
}

impl<T> Clone for RefKindVariable<T> {
    fn clone(&self) -> Self {
        Self {
            var: self.var,
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
    fn variant_for<V>(var: Variable<V>) -> RefKindVariable<V> {
        RefKindVariable {
            var,
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


pub trait TaskParameters: Clone + Component {
    fn register_vars(&self, task: TaskDyn, world: &mut World);
}

pub trait TaskParam {
    type Vars: TaskParameters;
}

#[derive(Clone)]
pub struct TaskParameter0{}
impl TaskParameters for TaskParameter0 {
    fn register_vars(&self, _task: TaskDyn, _world: &mut World) {
    }
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

pub struct Task<Vars: TaskParam> {
    f: unsafe fn(),
    dyn_caller: unsafe fn(unsafe fn(), TaskDataDyn, &mut World),
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
    dyn_caller: unsafe fn(unsafe fn(), TaskDataDyn, &mut World),
    v: TaskDataDyn,
}


impl<T: TaskParam> Task<T> {
    pub fn call_from_world(self, world: &mut World) {
        unsafe {
            (self.dyn_caller)(self.f, self.v.erase(), world);
        }
    }

    pub fn erase(self) -> TaskDyn {
        TaskDyn {
            f: self.f,
            dyn_caller: self.dyn_caller,
            v: self.v.erase(),
        }
    }
}

impl TaskDyn {
    pub fn call_from_world(self, world: &mut World) {
        unsafe {
            (self.dyn_caller)(self.f, self.v, world);
        }
    }
}

unsafe fn call_from_world_dyn0(f: unsafe fn(), variables: TaskDataDyn, world: &mut World) {
    let exact_f: fn() = unsafe { std::mem::transmute(f) };
    let Some(exact_variables) = variables.downcast::<<() as TaskParam>::Vars>() else {eprintln!("unexpected typeid"); return};
    exact_f.call_from_world(exact_variables, world);
}

impl Task<()> {
    pub fn new0(f: fn(), world: &mut World) -> Self {
        Self {
            f: unsafe { std::mem::transmute(f) },
            dyn_caller: call_from_world_dyn0,
            v: world.insert_task(TaskParameter0 {  }),
        }
    }
}


macro_rules! impl_call_from_world_dyn {
    ($fn_name:ident -> $task_new:ident -> $task_param:ident -> $($t:ident-$r:ident-$v:ident-$f:ident),+) => {

        pub struct $task_param<$( $t: Component ),+> {
            $($f: RefKindVariable<$t>),+
        }

        impl<$( $t: Component ),+> Clone for $task_param<$( $t ),+> {
            fn clone(&self) -> Self {
                Self {
                    $($f: self.$f.clone()),+
                }
            }
        }
        impl<$( $t: Component ),+> TaskParameters for $task_param<$( $t ),+> {
            fn register_vars(&self, task: TaskDyn, world: &mut World) {
                $(
                    if self.$f.ref_kind == RefKindVariant::Ref {
                        world.register_task_for_var(task, self.$f.var);
                    }
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
                    let $f = a.$f.var.get_mut(world);
                )+
                let inner = || {
                    $(
                        let Some($f) = $f else {return true};
                    )+
                    (self)(
                        $(
                            unsafe { $r::from_ref($f.0) }
                        ),+
                    );
                    $(
                        a.$f.var.reset_mut(world);
                    )+
                    $(
                        if a.$f.ref_kind == RefKindVariant::Mut {
                            for d in &$f.1 {
                                d.call_from_world(world);
                            }
                        }
                    )+
                    return false;
                };
                if inner() {
                    $(
                        a.$f.var.reset_mut(world);
                    )+
                }
            }
        }

        #[allow(dead_code)]
        unsafe fn $fn_name<$($t: Component, $r: RefKind<$t>),+>(
            f: unsafe fn(),
            variables: TaskDataDyn,
            world: &mut World,
        ) {
            let exact_f: fn($($r),+) = unsafe { std::mem::transmute(f) };
            let Some(exact_variables) = variables.downcast::<<($($t),+,) as TaskParam>::Vars>() else {eprintln!("unexpected typeid"); return};
            exact_f.call_from_world(exact_variables, world);
        }

        #[allow(dead_code)]
        impl<$($t: Component),+> Task<($($t),+,)> {
            pub fn $task_new<$($r: RefKind<$t>),+>(
                f: fn($($r),+),
                $($v: Variable<$t>,)+
                world: &mut World,
            ) -> Self {
                Self {
                    f: unsafe { std::mem::transmute(f) },
                    dyn_caller: $fn_name::<$($t, $r),+>,
                    v: world.insert_task($task_param {
                        $($f: $r::variant_for($v),)+
                    }),
                }
            }
        }
    };
}

impl_call_from_world_dyn!(call_from_world_dyn1 -> new1 -> TaskParameter1 -> T0-R0-v0-f0);
impl_call_from_world_dyn!(call_from_world_dyn2 -> new2 -> TaskParameter2 -> T0-R0-v0-f0, T1-R1-v1-f1);
impl_call_from_world_dyn!(call_from_world_dyn3 -> new3 -> TaskParameter3 -> T0-R0-v0-f0, T1-R1-v1-f1, T2-R2-v2-f2);

