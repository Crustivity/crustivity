/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crustivity_core::{
    constraints::ConstraintSystem, Effect, Event, Mut, Ref, WorldBuilder, WorldDataCreator,
};
use crustivity_ui::interaction::{Init, MouseClick};
use vello::{
    kurbo::Affine,
    peniko::{Color, Fill},
    Scene, SceneBuilder,
};

fn main() {
    let world = WorldBuilder::new();
    let mut system = ConstraintSystem::new();

    fn render(x: f64, y: f64, scene: &mut Scene) {
        let mut builder = SceneBuilder::for_scene(scene);
        use vello::kurbo::PathEl::*;
        let shape = [
            MoveTo((0.0, 0.0).into()),
            LineTo((100.0, 0.0).into()),
            LineTo((x, y).into()),
            ClosePath,
        ];
        builder.fill(
            Fill::NonZero,
            Affine::scale(1.0),
            Color::rgb(1.0, 1.0, 0.0),
            None,
            &shape,
        );
    }

    let on_init = world.task2(
        |_: Ref<_>, mut scene: Mut<_>| render(100.0, 100.0, &mut scene),
        Event::<Init>,
        Effect::<Scene>,
    );
    system.add_constraint(world.constraint(on_init));

    let on_click = world.task2(
        |pos: Ref<_>, mut scene: Mut<_>| render(pos.0.x, pos.0.y, &mut scene),
        Event::<MouseClick>,
        Effect::<Scene>,
    );
    system.add_constraint(world.constraint(on_click));

    let world = world.build(system);
    crustivity_ui::run(world);
}
