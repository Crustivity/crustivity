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

trait Component: Send + 'static {}
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
    data: Vec<TrackedVariable<T>>,
    gen: Vec<(usize, bool)>,
}

struct TaskTable<T> {
    data: Vec<T>,
    gen: Vec<usize>,
}

impl<T> VarTable<T> {
    fn new() -> Self {
        VarTable {
            data: Vec::new(),
            gen: Vec::new(),
        }
    }
}

impl<T> TaskTable<T> {
    fn new() -> Self {
        TaskTable {
            data: Vec::new(),
            gen: Vec::new(),
        }
    }
}

struct World {
    vars: HashMap<TypeId, Box<dyn Any + Send>>,
    tasks: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl World {
    fn new() -> Self {
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
        table.data.push(TrackedVariable::new(t));
        table.gen.push((0, false));
        Variable {
            index: table.data.len() - 1,
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
        table.data.push(t);
        table.gen.push(0);
        TaskData {
            index: table.data.len() - 1,
            generation: 0,
            _t: PhantomData::default(),
        }
    }

    fn effect<T: CrusTuple>(&mut self, task: Task<T>) {
        let Some(task_data) = task.v.get(self) else {return};
        task_data.register_vars(task.erase(), self);
    }
}

struct Variable<T> {
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
    fn new(t: T, world: &mut World) -> Self {
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
        let var = &table.data[self.index]; 
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

    fn set(self, t: T, world: &mut World) {
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
        let var = &mut table.data[self.index];
        *var.variable.get_mut() = t;
        for d in var.dependents.clone() {
            d.call_from_world(world);
        }
    }

    fn print_meta_data(self, world: &mut World) {
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
                table.data[self.index].dependents.len(),
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
enum RefKindVariant {
    Ref,
    Mut,
}

struct RefKindVariable<T> {
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
        Some(table.data[self.index].clone())
    }
}

trait RefKind<T: Component> {
    unsafe fn from_ref(t: *mut T) -> Self;
    fn variant() -> RefKindVariant;
    fn variant_for<V>(var: Variable<V>) -> RefKindVariable<V> {
        RefKindVariable {
            var,
            ref_kind: Self::variant(),
        }
    }
}

struct Ref<'a, T: Component>(&'a T);
struct Mut<'a, T: Component>(&'a mut T);

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

trait TaskParameters: Clone + Component {
    fn register_vars(&self, task: TaskDyn, world: &mut World);
}

impl TaskParameters for () {
    fn register_vars(&self, _task: TaskDyn, _world: &mut World) {}
}

impl<T: Component> TaskParameters for (RefKindVariable<T>,) {
    fn register_vars(&self, task: TaskDyn, world: &mut World) {
        if self.0.ref_kind == RefKindVariant::Ref {
            world.register_task_for_var(task, self.0.var);
        }
    }
}

impl<T0: Component, T1: Component> TaskParameters for (RefKindVariable<T0>, RefKindVariable<T1>) {
    fn register_vars(&self, task: TaskDyn, world: &mut World) {
        if self.0.ref_kind == RefKindVariant::Ref {
            world.register_task_for_var(task, self.0.var);
        }
        if self.1.ref_kind == RefKindVariant::Ref {
            world.register_task_for_var(task, self.1.var);
        }
    }
}

trait CrusTuple {
    type Vars: TaskParameters;
}

impl CrusTuple for () {
    type Vars = ();
}

impl<T: Component> CrusTuple for (T,) {
    type Vars = (RefKindVariable<T>,);
}

impl<T0: Component, T1: Component> CrusTuple for (T0, T1) {
    type Vars = (RefKindVariable<T0>, RefKindVariable<T1>);
}

trait Call<Vars: CrusTuple> {
    fn call_from_world(self, a: TaskData<Vars::Vars>, world: &mut World);
}

impl Call<()> for fn() {
    fn call_from_world(self, _a: TaskData<()>, _world: &mut World) {
        (self)();
    }
}

impl<T: Component, R: RefKind<T>> Call<(T,)> for fn(R) {
    fn call_from_world(self, a: TaskData<<(T,) as CrusTuple>::Vars>, world: &mut World) {
        let Some(a) = a.get(world) else {return};
        let arg0 = a.0.var.get_mut(world);
        if let Some((arg0, arg0_deps)) = arg0 {
            (self)(unsafe { R::from_ref(arg0) });
            a.0.var.reset_mut(world);
            if a.0.ref_kind == RefKindVariant::Mut {
                for d in arg0_deps {
                    d.call_from_world(world);
                }
            }
            return;
        }
        a.0.var.reset_mut(world);
    }
}

impl<T0: Component, T1: Component, R0: RefKind<T0>, R1: RefKind<T1>> Call<(T0, T1)> for fn(R0, R1) {
    fn call_from_world(self, a: TaskData<<(T0, T1) as CrusTuple>::Vars>, world: &mut World) {
        let Some(a) = a.get(world) else {return};
        let arg0 = a.0.var.get_mut(world);
        let arg1 = a.1.var.get_mut(world);
        if let Some((arg0, arg0_deps)) = arg0 {
            if let Some((arg1, arg1_deps)) = arg1 {
                (self)(unsafe { R0::from_ref(arg0) }, unsafe { R1::from_ref(arg1) });
                a.1.var.reset_mut(world);
                a.0.var.reset_mut(world);
                if a.0.ref_kind == RefKindVariant::Mut {
                    for d in arg0_deps {
                        d.call_from_world(world);
                    }
                }
                if a.1.ref_kind == RefKindVariant::Mut {
                    for d in arg1_deps {
                        d.call_from_world(world);
                    }
                }
                return;
            }
        }
        a.1.var.reset_mut(world);
        a.0.var.reset_mut(world);
    }
}

struct Task<Vars: CrusTuple> {
    f: unsafe fn(),
    dyn_caller: unsafe fn(unsafe fn(), TaskDataDyn, &mut World),
    v: TaskData<Vars::Vars>,
}

impl<Vars: CrusTuple> Clone for Task<Vars> {
    fn clone(&self) -> Self {
        Self {
            f: self.f,
            dyn_caller: self.dyn_caller,
            v: self.v,
        }
    }
}

impl<Vars: CrusTuple> Copy for Task<Vars> {}

#[derive(Clone, Copy)]
struct TaskDyn {
    f: unsafe fn(),
    dyn_caller: unsafe fn(unsafe fn(), TaskDataDyn, &mut World),
    v: TaskDataDyn,
}

unsafe fn call_from_world_dyn0(f: unsafe fn(), variables: TaskDataDyn, world: &mut World) {
    let exact_f: fn() = unsafe { std::mem::transmute(f) };
    let Some(exact_variables) = variables.downcast::<<() as CrusTuple>::Vars>() else {eprintln!("unexpected typeid"); return};
    exact_f.call_from_world(exact_variables, world);
}

unsafe fn call_from_world_dyn1<T: Component, R: RefKind<T>>(
    f: unsafe fn(),
    variables: TaskDataDyn,
    world: &mut World,
) {
    let exact_f: fn(R) = unsafe { std::mem::transmute(f) };
    let Some(exact_variables) = variables.downcast::<<(T,) as CrusTuple>::Vars>() else {eprintln!("unexpected typeid"); return};
    exact_f.call_from_world(exact_variables, world);
}

unsafe fn call_from_world_dyn2<T0: Component, T1: Component, R0: RefKind<T0>, R1: RefKind<T1>>(
    f: unsafe fn(),
    variables: TaskDataDyn,
    world: &mut World,
) {
    let exact_f: fn(R0, R1) = unsafe { std::mem::transmute(f) };
    let Some(exact_variables) = variables.downcast::<<(T0, T1) as CrusTuple>::Vars>() else {eprintln!("unexpected typeid"); return};
    exact_f.call_from_world(exact_variables, world);
}

impl<T: CrusTuple> Task<T> {
    fn call_from_world(self, world: &mut World) {
        unsafe {
            (self.dyn_caller)(self.f, self.v.erase(), world);
        }
    }

    fn erase(self) -> TaskDyn {
        TaskDyn {
            f: self.f,
            dyn_caller: self.dyn_caller,
            v: self.v.erase(),
        }
    }
}

impl TaskDyn {
    fn call_from_world(self, world: &mut World) {
        unsafe {
            (self.dyn_caller)(self.f, self.v, world);
        }
    }
}

impl Task<()> {
    fn new0(f: fn(), world: &mut World) -> Self {
        Self {
            f: unsafe { std::mem::transmute(f) },
            dyn_caller: call_from_world_dyn0,
            v: world.insert_task(()),
        }
    }
}

impl<T: Component> Task<(T,)> {
    fn new1<R: RefKind<T>>(f: fn(R), v: Variable<T>, world: &mut World) -> Self {
        Self {
            f: unsafe { std::mem::transmute(f) },
            dyn_caller: call_from_world_dyn1::<T, R>,
            v: world.insert_task((R::variant_for(v),)),
        }
    }
}

impl<T0: Component, T1: Component> Task<(T0, T1)> {
    fn new2<R0: RefKind<T0>, R1: RefKind<T1>>(
        f: fn(R0, R1),
        v0: Variable<T0>,
        v1: Variable<T1>,
        world: &mut World,
    ) -> Self {
        Self {
            f: unsafe { std::mem::transmute(f) },
            dyn_caller: call_from_world_dyn2::<T0, T1, R0, R1>,
            v: world.insert_task((R0::variant_for(v0), R1::variant_for(v1))),
        }
    }
}

fn main() {
    let mut world = World::new();
    let world = &mut world;
    let a = Variable::new("42".to_string(), world);
    let b = Variable::new(42, world);

    fn inc_num(mut b: Mut<i32>) {
        *b += 1;
    }

    fn format_num(mut s: Mut<String>, n: Ref<i32>) {
        *s = n.to_string();
    }

    fn print(a: Ref<String>) {
        println!("string: {}", *a);
    }

    fn check() {
        println!("Hello World");
    }

    let check = Task::new0(check, world).erase();
    let inc_num = Task::new1(inc_num, b, world).erase();
    let format_num = Task::new2(format_num, a, b, world);
    let print = Task::new1(print, a, world);

    world.effect(print);
    world.effect(format_num);

    print.call_from_world(world);
    inc_num.call_from_world(world);
    inc_num.call_from_world(world);
    inc_num.call_from_world(world);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {}
}
