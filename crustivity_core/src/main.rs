/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crustivity_core::{
    constraints::System, Component, Effect, Event, Mut, Ref, WorldBuilder, WorldDataCreator,
};

use std::fmt::Write;

struct IncrementW;
struct IncrementH;
struct IncrementT;

#[derive(Default, Debug)]
struct PrintEffect(String);

fn main() {
    let world = WorldBuilder::new();

    world.register_effect_named::<PrintEffect>("Print Effect");
    world
        .register_event_named(IncrementW, "Increment Width")
        .ok()
        .unwrap();
    world
        .register_event_named(IncrementH, "Increment Height")
        .ok()
        .unwrap();
    world
        .register_event_named(IncrementT, "Increment Total")
        .ok()
        .unwrap();

    let width_str = world.variable_named("2".to_string(), "width_str");
    let height_str = world.variable_named("4".to_string(), "height_str");
    let total_str = world.variable_named("8".to_string(), "total_str");

    let width_num = world.variable_named(2f64, "width_num");
    let height_num = world.variable_named(4f64, "height_num");
    let total_num = world.variable_named(8f64, "total_num");

    let width_out = world.task3_named(
        "w ⟵ t / h",
        |mut w: Mut<_>, t: Ref<_>, h: Ref<_>| {
            *w = *t / *h;
        },
        width_num,
        total_num,
        height_num,
    );
    let height_out = world.task3_named(
        "h ⟵ t / w",
        |mut h: Mut<_>, t: Ref<_>, w: Ref<_>| {
            *h = *t / *w;
        },
        height_num,
        total_num,
        width_num,
    );
    let total_out = world.task3_named(
        "t ⟵ w · h",
        |mut t: Mut<_>, w: Ref<_>, h: Ref<_>| {
            *t = *w * *h;
        },
        total_num,
        width_num,
        height_num,
    );

    fn num_to_str_fn(num: Ref<f64>, mut num_str: Mut<String>) {
        // this is done in a stupid way, `*num_str = num.to_string()` would do it,
        // but doing it this way shows that custivity does not relay on setting a new value like many other declarative framworks
        num_str.clear();
        write!(&mut *num_str, "{}", *num).unwrap();
    }

    let width_to_str = world.task2_named("width_to_str", num_to_str_fn, width_num, width_str);
    let height_to_str = world.task2_named("height_to_str", num_to_str_fn, height_num, height_str);
    let total_to_str = world.task2_named("total_to_str", num_to_str_fn, total_num, total_str);

    fn inc_num<E: Component>(_: Ref<E>, mut num: Mut<f64>) {
        *num += 1.0;
    }

    let inc_width = world.task2_named("width++", inc_num, Event::<IncrementW>, width_num);
    let inc_height = world.task2_named("height++", inc_num, Event::<IncrementH>, height_num);
    let inc_total = world.task2_named("total++", inc_num, Event::<IncrementT>, total_num);

    let print_nums = world.task4_named(
        "print_nums",
        |mut res: Mut<_>, w: Ref<_>, h: Ref<_>, t: Ref<_>| {
            res.0 = format!("t:{} = w:{} * h:{}", *t, *w, *h);
        },
        Effect::<PrintEffect>,
        width_str,
        height_str,
        total_str,
    );

    let mut system = System::default();

    system.add_constraint(
        world
            .constraint(width_out)
            .add_method(height_out)
            .add_method(total_out)
            .name("total = width · height"),
    );
    system.add_constraint(world.constraint(width_to_str).name("width_to_str"));
    system.add_constraint(world.constraint(height_to_str).name("height_to_str"));
    system.add_constraint(world.constraint(total_to_str).name("total_to_str"));

    system.add_constraint(world.constraint(inc_width).name("inc_event_w"));
    system.add_constraint(world.constraint(inc_height).name("inc_event_h"));
    system.add_constraint(world.constraint(inc_total).name("inc_event_t"));

    system.add_constraint(world.constraint(print_nums).name("print"));

    system.write_graphvis(&world, "pre.svg");

    let world = world.build(system);
}
