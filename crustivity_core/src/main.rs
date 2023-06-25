/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crustivity_core::{
    spawner::Spawner,
    Mut, Ref, Resource, Task, Variable, World,
};

fn main() {
    let mut world = World::new();
    {
        let world = &mut world;


        let a = Variable::new("42".to_string(), world);
        let b = Variable::new(42, world);

        fn inc_num(mut b: Mut<i32>) {
            *b += 1;
        }

        fn format_num(mut s: Mut<String>, n: Ref<i32>) {
            *s = n.to_string();
        }

        fn check() {
            println!("Hello World");
        }

        let check = Task::new0(check, world).erase();
        let inc_num = Task::new1(inc_num, b, world).erase();
        let format_num = Task::new2(format_num, a, b, world);
        let print = Task::new1(|s: Ref<_>| println!("{}", *s), a, world);

        check.call_from_world(world);
        print.call_from_world(world);
        inc_num.call_from_world(world);
        inc_num.call_from_world(world);
        inc_num.call_from_world(world);
    }
    fn init(system: Ref<Spawner>) {
        println!("Init");
    }
    let init = Task::new1(init, Resource::<Spawner>, &mut world);
    world.start(init.erase());
}
