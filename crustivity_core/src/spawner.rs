/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::{borrow::Cow, sync::Arc};

use crate::{
    constraints::ConstraintBuilder, world::World, Component, Task, TaskData, Variable,
    WorldDataCreator,
};

pub struct Spawner {
    pub(crate) world: Arc<World>,
}

impl Spawner {
    pub(crate) fn new(world: Arc<World>) -> Self {
        Self { world }
    }

    pub fn emit_event<T: Component>(&self, t: T) {
        self.world.emit_event(t)
    }
}

impl WorldDataCreator for Spawner {
    fn variable<T: Component>(&self, t: T) -> Variable<T> {
        self.world.variable(t)
    }

    fn variable_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Variable<T> {
        self.world.variable_named(t, name)
    }

    fn resource<T: Component>(&self, t: T) -> Result<(), T> {
        self.world.resource(t)
    }

    fn resource_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Result<(), T> {
        self.world.resource_named(t, name)
    }

    fn constraint<T: TaskData>(&self, task: Task<T>) -> ConstraintBuilder {
        self.world.constraint(task)
    }

    fn register_effect<T: Component + Default>(&self) {
        self.world.register_effect::<T>()
    }

    fn register_effect_named<T: Component + Default>(&self, name: impl Into<Cow<'static, str>>) {
        self.world.register_effect_named::<T>(name)
    }

    fn register_event<T: Component>(&self, t: T) -> Result<(), T> {
        self.world.register_event(t)
    }

    fn register_event_named<T: Component>(
        &self,
        t: T,
        name: impl Into<Cow<'static, str>>,
    ) -> Result<(), T> {
        self.world.register_event_named(t, name)
    }
}
