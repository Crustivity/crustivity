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
};

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

    fn insert<T: Send + 'static>(&mut self, t: T) -> Variable<T> {
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

impl<T: Send + 'static> Variable<T> {
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

trait CrusTuple {
    type Vars: Clone;
}

impl<T: Send + 'static> CrusTuple for (T,) {
    type Vars = (Variable<T>,);
}

impl<T0: Send + 'static, T1: Send + 'static> CrusTuple for (T0, T1) {
    type Vars = (Variable<T0>, Variable<T1>);
}

trait Call<Vars: CrusTuple> {
    fn call_from_world(self, a: <Vars as CrusTuple>::Vars, world: &mut World);
}

// impl<Func: Fn()> Call<()> for Func {
//     fn call_from_world(self, a: (), world: &mut World) {
//         (self)();
//     }
// }

impl<T: Send + 'static> Call<(T,)> for fn(&mut T) {
    fn call_from_world(self, a: <(T,) as CrusTuple>::Vars, world: &mut World) {
        let arg0 = a.0.get_mut(world);
        if let Some(arg0) = arg0 {
            (self)(unsafe { &mut *arg0 });
        }
        a.0.reset_mut(world);
    }
}

impl<T0: Send + 'static, T1: Send + 'static> Call<(T0, T1)> for fn(&mut T0, &mut T1) {
    fn call_from_world(self, a: (Variable<T0>, Variable<T1>), world: &mut World) {
        let arg0 = a.0.get_mut(world);
        let arg1 = a.1.get_mut(world);
        if let Some(arg0) = arg0 {
            if let Some(arg1) = arg1 {
                (self)(unsafe { &mut *arg0 }, unsafe { &mut *arg1 });
            }
        }
        a.1.reset_mut(world);
        a.0.reset_mut(world);
    }
}

struct Task<Vars: CrusTuple> {
    f: unsafe fn(),
    dyn_caller: unsafe fn(unsafe fn(), <Vars as CrusTuple>::Vars, &mut World),
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

unsafe fn call_from_world_dyn1<T: Send + 'static>(
    f: unsafe fn(),
    v: (Variable<T>,),
    world: &mut World,
) {
    let exact_f: fn(&mut T) = unsafe { std::mem::transmute(f) };
    exact_f.call_from_world(v, world);
}

unsafe fn call_from_world_dyn2<T0: Send + 'static, T1: Send + 'static>(
    f: unsafe fn(),
    v: (Variable<T0>, Variable<T1>),
    world: &mut World,
) {
    let exact_f: fn(&mut T0, &mut T1) = unsafe { std::mem::transmute(f) };
    exact_f.call_from_world(v, world);
}

impl<T: CrusTuple> Task<T> {
    fn call_from_world(&self, world: &mut World) {
        unsafe {
            (self.dyn_caller)(self.f, self.v.clone(), world);
        }
    }
}

impl<T: Send + 'static> Task<(T,)> {
    fn new1(f: fn(&mut T), v: Variable<T>) -> Self {
        Self {
            f: unsafe { std::mem::transmute(f) },
            dyn_caller: call_from_world_dyn1::<T>,
            v: (v,),
        }
    }
}
impl<T0: Send + 'static, T1: Send + 'static> Task<(T0, T1)> {
    fn new2(f: fn(&mut T0, &mut T1), v0: Variable<T0>, v1: Variable<T1>) -> Self {
        Self {
            f: unsafe { std::mem::transmute(f) },
            dyn_caller: call_from_world_dyn2::<T0, T1>,
            v: (v0, v1),
        }
    }
}

fn main() {
    let mut world = World::new();
    let a = Variable::new("Test".to_string(), &mut world);
    let b = Variable::new(42, &mut world);

    fn append_str(a: &mut String) {
        a.push_str(" Test");
    }
    fn inc_num(b: &mut i32) {
        *b += 1;
    }

    fn print(a: &mut String, b: &mut i32) {
        println!("a: {a}, b: {b}");
    }

    let inc_num = Task::new1(inc_num, b);
    let append_str = Task::new1(append_str, a);
    let print = Task::new2(print, a, b);

    print.call_from_world(&mut world);
    inc_num.call_from_world(&mut world);
    append_str.call_from_world(&mut world);
    print.call_from_world(&mut world);

    // (print as fn(&mut String, &mut i32)).call_from_world((a, b), &mut world);
    // call_from_world(print, (a, b), &mut world);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {}
}
