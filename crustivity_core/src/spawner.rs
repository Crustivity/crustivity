/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::Arc;

use crate::{
    constraints::{Constraint, Effect},
    world::World,
    Component, Task, TaskParam, Variable,
};

pub struct Spawner {
    pub(crate) world: Arc<World>,
}

impl Spawner {
    pub(crate) fn new(world: Arc<World>) -> Self {
        Self { world }
    }

    pub fn variable<T: Component>(&self, t: T) -> Variable<T> {
        self.world.insert_var(t)
    }

    pub fn emit_event<T: Component>(&self, t: T) {
        self.world.emit_event(t)
    }

    pub fn resource<T: Component>(&self, t: T) -> Result<(), T> {
        self.world.insert_resource(t)
    }

    pub fn constraint<T: TaskParam>(&self, task: Task<T>) -> Constraint {
        Constraint::new(task, &self.world)
    }

    pub fn effect<T: TaskParam>(&self, task: Task<T>) -> Effect {
        Effect::new(task, &self.world)
    }
}
