/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crustivity_core::{constraints::ConstraintSystem, Effect, Event, Mut, Ref, WorldBuilder};

use std::fmt::Write;

struct Increment;

#[derive(Default, Debug)]
struct TestEffect(isize);

fn main() {
    let world = WorldBuilder::new();

    world.register_effect::<TestEffect>();

    let my_str = world.variable("0".to_string());
    let my_num = world.variable(0isize);

    let num_to_str = world.task2(
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

    let inc_event = world.task2(
        |_: Ref<_>, mut num: Mut<_>| {
            *num += 1;
            println!("Increment");
        },
        Event::<Increment>,
        my_num,
    );

    let eff = world.task2(
        |num: Ref<_>, mut eff: Mut<_>| {
            eff.0 = *num;
        },
        my_num,
        Effect::<TestEffect>,
    );

    let mut system = ConstraintSystem::new();

    system.add_constraint(world.constraint(num_to_str));
    system.add_constraint(world.constraint(inc_event));
    system.add_constraint(world.constraint(eff));

    let world = world.build(system);

    world.emit_event(Increment);
    world.process_next_activation();

    let res = world.receiv_effects::<TestEffect>();

    println!("{res:#?}");

    world.emit_event(Increment);
    world.process_next_activation();

    let res = world.receiv_effects::<TestEffect>();

    println!("{res:#?}");
}
