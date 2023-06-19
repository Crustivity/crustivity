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

struct Table<T> {
    data: Vec<UnsafeCell<T>>,
    gen: Vec<(usize, bool)>,
}
impl<T> Table<T> {
    fn new() -> Self {
        Table {
            data: Vec::new(),
            gen: Vec::new(),
        }
    }
}

struct World(HashMap<TypeId, Box<dyn Any + Send>>);

impl World {
    fn new() -> Self {
        World(HashMap::new())
    }

    fn insert<T: Component>(&mut self, t: T) -> Variable<T> {
        let table = self
            .0
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Table::<T>::new()) as Box<dyn Any + Send>)
            .as_mut()
            .downcast_mut::<Table<T>>()
            .expect("Any to be a Table<T>");
        table.data.push(UnsafeCell::new(t));
        table.gen.push((0, false));
        Variable {
            index: table.data.len() - 1,
            generation: 0,
            _t: PhantomData::default(),
        }
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
        world.insert(t)
    }

    fn get_mut(self, world: &mut World) -> Option<*mut T> {
        let table = world
            .0
            .get_mut(&TypeId::of::<T>())?
            .downcast_mut::<Table<T>>()
            .expect("any to be Table<T>");
        let (gen, in_use) = table.gen.get_mut(self.index)?;
        if *gen != self.generation {
            return None;
        }
        if *in_use {
            return None;
        }
        *in_use = true;
        Some(table.data[self.index].get())
    }

    fn reset_mut(self, world: &mut World) {
        let Some(table) = world
            .0
            .get_mut(&TypeId::of::<T>()) else {return};
        let table = table
            .downcast_mut::<Table<T>>()
            .expect("any to be Table<T>");
        let Some((gen, in_use)) = table.gen.get_mut(self.index) else {return};
        if *gen != self.generation {
            return;
        }
        *in_use = false;
    }
}

trait RefKind<T: Component> {
    unsafe fn from_ref(t: *mut T) -> Self;
}

struct Ref<'a, T: Component>(&'a T);
struct Mut<'a, T: Component>(&'a mut T);

impl<'a, T: Component> RefKind<T> for Ref<'a, T> {
    unsafe fn from_ref(t: *mut T) -> Self {
        Self(unsafe { &*t })
    }
}
impl<'a, T: Component> RefKind<T> for Mut<'a, T> {
    unsafe fn from_ref(t: *mut T) -> Self {
        Self(unsafe { &mut *t })
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

trait CrusTuple {
    type Vars: Clone;
}

impl CrusTuple for () {
    type Vars = ();
}

impl<T: Component> CrusTuple for (T,) {
    type Vars = (Variable<T>,);
}

impl<T0: Component, T1: Component> CrusTuple for (T0, T1) {
    type Vars = (Variable<T0>, Variable<T1>);
}

trait Call<Vars: CrusTuple> {
    fn call_from_world(self, a: Vars::Vars, world: &mut World);
}

impl Call<()> for fn() {
    fn call_from_world(self, _a: (), _world: &mut World) {
        (self)();
    }
}

impl<T: Component, R: RefKind<T>> Call<(T,)> for fn(R) {
    fn call_from_world(self, a: <(T,) as CrusTuple>::Vars, world: &mut World) {
        let arg0 = a.0.get_mut(world);
        if let Some(arg0) = arg0 {
            (self)(unsafe { R::from_ref(arg0) });
        }
        a.0.reset_mut(world);
    }
}

impl<T0: Component, T1: Component, R0: RefKind<T0>, R1: RefKind<T1>> Call<(T0, T1)> for fn(R0, R1) {
    fn call_from_world(self, a: <(T0, T1) as CrusTuple>::Vars, world: &mut World) {
        let arg0 = a.0.get_mut(world);
        let arg1 = a.1.get_mut(world);
        if let Some(arg0) = arg0 {
            if let Some(arg1) = arg1 {
                (self)(unsafe { R0::from_ref(arg0) }, unsafe { R1::from_ref(arg1) });
            }
        }
        a.1.reset_mut(world);
        a.0.reset_mut(world);
    }
}

struct Task<Vars: CrusTuple> {
    f: unsafe fn(),
    dyn_caller: unsafe fn(unsafe fn(), Vars::Vars, &mut World),
    v: <Vars as CrusTuple>::Vars,
}

impl<Vars: CrusTuple> Clone for Task<Vars> {
    fn clone(&self) -> Self {
        Self {
            f: self.f,
            dyn_caller: self.dyn_caller,
            v: self.v.clone(),
        }
    }
}

unsafe fn call_from_world_dyn0(f: unsafe fn(), v: (), world: &mut World) {
    let exact_f: fn() = unsafe { std::mem::transmute(f) };
    exact_f.call_from_world(v, world);
}

unsafe fn call_from_world_dyn1<T: Component, R: RefKind<T>>(
    f: unsafe fn(),
    v: (Variable<T>,),
    world: &mut World,
) {
    let exact_f: fn(R) = unsafe { std::mem::transmute(f) };
    exact_f.call_from_world(v, world);
}

unsafe fn call_from_world_dyn2<T0: Component, T1: Component, R0: RefKind<T0>, R1: RefKind<T1>>(
    f: unsafe fn(),
    v: (Variable<T0>, Variable<T1>),
    world: &mut World,
) {
    let exact_f: fn(R0, R1) = unsafe { std::mem::transmute(f) };
    exact_f.call_from_world(v, world);
}

impl<T: CrusTuple> Task<T> {
    fn call_from_world(&self, world: &mut World) {
        unsafe {
            (self.dyn_caller)(self.f, self.v.clone(), world);
        }
    }
}

impl Task<()> {
    fn new0(f: fn()) -> Self {
        Self {
            f: unsafe { std::mem::transmute(f) },
            dyn_caller: call_from_world_dyn0,
            v: (),
        }
    }
}

impl<T: Component> Task<(T,)> {
    fn new1<R: RefKind<T>>(f: fn(R), v: Variable<T>) -> Self {
        Self {
            f: unsafe { std::mem::transmute(f) },
            dyn_caller: call_from_world_dyn1::<T, R>,
            v: (v,),
        }
    }
}

impl<T0: Component, T1: Component> Task<(T0, T1)> {
    fn new2<R0: RefKind<T0>, R1: RefKind<T1>>(
        f: fn(R0, R1),
        v0: Variable<T0>,
        v1: Variable<T1>,
    ) -> Self {
        Self {
            f: unsafe { std::mem::transmute(f) },
            dyn_caller: call_from_world_dyn2::<T0, T1, R0, R1>,
            v: (v0, v1),
        }
    }
}

fn main() {
    let mut world = World::new();
    let world = &mut world;
    let a = Variable::new("Test".to_string(), world);
    let b = Variable::new(42, world);

    fn append_str(mut a: Mut<String>) {
        a.push_str(" Test");
    }

    fn inc_num(mut b: Mut<i32>) {
        *b += 1;
    }

    fn print(a: Ref<String>, b: Ref<i32>) {
        println!("a: {}, b: {}", *a, *b);
    }

    fn check() {
        println!("Hello World");
    }

    let check = Task::new0(check);
    let inc_num = Task::new1(inc_num, b);
    let append_str = Task::new1(append_str, a);
    let print = Task::new2(print, a, b);

    check.call_from_world(world);
    print.call_from_world(world);
    inc_num.call_from_world(world);
    append_str.call_from_world(world);
    print.call_from_world(world);

    // (print as fn(&mut String, &mut i32)).call_from_world((a, b), &mut world);
    // call_from_world(print, (a, b), &mut world);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {}
}
