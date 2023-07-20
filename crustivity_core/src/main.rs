/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crustivity_core::{
    constraints::ConstraintSystem, spawner::Spawner, Event, Mut, Ref, Resource, World,
};

use std::fmt::Write;

struct Increment;

fn main() {
    let world = World::new();

    fn init(spawner: Ref<Spawner>, mut system: Mut<ConstraintSystem>) {
        println!("Init");

        let my_str = spawner.variable("0".to_string());
        let my_num = spawner.variable(0isize);

        let num_to_str = spawner.task2(
            |num: Ref<isize>, mut num_str: Mut<String>| {
                // this is done in a stupid way, `*num_str = num.to_string()` would do it,
                // but doing it this way shows that custivity does not relay on setting a new value like many other declarative framworks
                num_str.clear();
                write!(&mut *num_str, "{}", *num).unwrap();
                println!("Convert");
            },
            my_num,
            my_str,
        );

        let inc_event = spawner.task2(
            |_: Ref<_>, mut num: Mut<_>| {
                *num += 1;
                println!("Increment");
            },
            Event::<Increment>,
            my_num,
        );

        let print_num = spawner.task1(|num: Ref<_>| println!("num: {}", *num), my_num);
        let print_str = spawner.task1(|s: Ref<_>| println!("str: {}", *s), my_str);

        system.add_constraint(spawner.constraint(num_to_str));
        system.add_constraint(spawner.constraint(inc_event));
        system.add_effect(spawner.effect(print_num));
        system.add_effect(spawner.effect(print_str));

        spawner.emit_event(Increment);
        spawner.emit_event(Increment);
    }

    let init = world.task2(init, Resource::<Spawner>, Resource::<ConstraintSystem>);
    world.start(init.erase());
}
