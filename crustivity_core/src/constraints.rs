/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use core::panic;
use std::{
    any::TypeId,
    collections::{HashMap, HashSet, VecDeque},
};

use crate::{
    Component, Task, TaskDyn, TaskParam, TaskParameter, TaskParameterDyn, VariableRegister, World,
};

#[derive(Default)]
struct Register(HashSet<TaskParameterDyn>, HashSet<TaskParameterDyn>);
impl VariableRegister for Register {
    fn register_var(&mut self, ref_kind: crate::RefKindVariant, var: TaskParameterDyn) {
        match ref_kind {
            crate::RefKindVariant::Ref => self.0.insert(var),
            crate::RefKindVariant::Mut => self.1.insert(var),
        };
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum TaskParameterStatus {
    #[default]
    In,
    Out,
}

#[derive(Clone)]
struct Vec2d<T> {
    vec: Vec<T>,
    row: usize,
    col: usize,
}

impl<T: Default + Clone> Vec2d<T> {
    fn new(row: Vec<T>) -> Self {
        let col = row.len();
        Self {
            vec: row,
            row: 1,
            col,
        }
    }

    fn push_row(&mut self, row: impl IntoIterator<Item = T>) {
        self.vec.extend(
            row.into_iter()
                .chain(std::iter::repeat(T::default()))
                .take(self.col),
        );
        self.row += 1;
    }

    fn iter_column(&self, column: usize) -> impl Iterator<Item = &T> {
        (0..self.row)
            .map(move |row| row * self.col + column)
            .map(|idx| &self.vec[idx])
    }

    fn row(&self, row: usize) -> &[T] {
        let i = self.col * row;
        &self.vec[i..(i + self.col)]
    }
}

pub struct Constraint {
    tasks: Vec<TaskDyn>,
    params: Vec<TaskParameterDyn>,

    /// 2d vect of parameter status
    /// a row contains all status from one task (identified by its index)
    /// and a column refers to which task parameter the status belongs to
    param_status: Vec2d<TaskParameterStatus>,
}

impl Constraint {
    pub fn new<T: TaskParam>(task: Task<T>, world: &World) -> Self {
        let mut register = Register::default();
        task.register_vars(&mut register, world);
        if !register.0.is_disjoint(&register.1) {
            panic!("Input and output variables have to be disjoint");
        }
        let mut param_status = Vec::with_capacity(register.0.len() + register.1.len());
        for _ in 0..register.0.len() {
            param_status.push(TaskParameterStatus::In);
        }
        for _ in 0..register.1.len() {
            param_status.push(TaskParameterStatus::Out);
        }
        let param_status = Vec2d::new(param_status);
        let params: Vec<TaskParameterDyn> = register
            .0
            .into_iter()
            .chain(register.1.iter().cloned())
            .collect();

        Self {
            tasks: vec![task.erase()],
            params,
            param_status,
        }
    }

    pub fn add_method<T: TaskParam>(mut self, task: Task<T>, world: &World) -> Self {
        let mut register = Register::default();
        task.register_vars(&mut register, world);
        if !register.0.is_disjoint(&register.1) {
            panic!("Input and output variables have to be disjoint");
        }
        let l = register.0.len() + register.1.len();
        if l != self.params.len() {
            panic!("This task does not use the same parameters than this constraint.");
        }
        let status = (0..l).map(|p_idx| &self.params[p_idx]).map(|task_param| {
            if register.0.contains(task_param) {
                TaskParameterStatus::In
            } else if register.1.contains(task_param) {
                TaskParameterStatus::Out
            } else {
                panic!("This task uses a parameter unknown to this constraint.")
            }
        });
        self.param_status.push_row(status);
        self
    }

    fn choose_method(
        &self,
        param: TaskParameterDyn,
    ) -> Option<(TaskDyn, impl Iterator<Item = TaskParameterDyn> + '_)> {
        let pos = self.params.iter().position(|&p| p == param)?;
        let chosen_row_idx = self
            .param_status
            .iter_column(pos)
            .position(|&s| s == TaskParameterStatus::In)?;
        let out_parameter = self
            .param_status
            .row(chosen_row_idx)
            .iter()
            .enumerate()
            .filter(|&(_, s)| *s == TaskParameterStatus::Out)
            .map(|c_idx| c_idx.0)
            .map(|c_idx| self.params[c_idx]);
        Some((self.tasks[chosen_row_idx], out_parameter))
    }
}

pub struct ConstraintSystem {
    constraints: Vec<Constraint>,
    params_to_constraints: HashMap<TaskParameterDyn, Vec<usize>>,
}

impl ConstraintSystem {
    pub fn new() -> Self {
        Self {
            constraints: Vec::new(),
            params_to_constraints: HashMap::new(),
        }
    }

    pub fn add_constraint(&mut self, constraint: Constraint) {
        let c_idx = self.constraints.len();
        for param in constraint.params.iter().copied() {
            self.params_to_constraints
                .entry(param)
                .or_insert_with(Vec::new)
                .push(c_idx);
        }
        self.constraints.push(constraint);
    }
}

pub(crate) struct EffectPath {
    pub(crate) tasks: Vec<TaskDyn>,
    pub(crate) effects: HashSet<TypeId>,
}

impl EffectPath {
    pub(crate) fn starting_with<T: Component>(
        param: impl Into<TaskParameter<T>>,
        system: &ConstraintSystem,
    ) -> Option<Self> {
        let param: TaskParameter<T> = param.into();
        EffectPath::starting_with_dyn(param.erase(), system)
    }

    pub(crate) fn starting_with_dyn(
        param: TaskParameterDyn,
        system: &ConstraintSystem,
    ) -> Option<Self> {
        let mut tasks = Vec::new();

        let mut q = VecDeque::<TaskParameterDyn>::new();
        q.push_back(param);

        let mut task_set = HashSet::new();
        let mut effects = HashSet::new();
        while let Some(param) = q.pop_front() {
            if let TaskParameterDyn::Effect(tid) = param {
                effects.insert(tid);
            }
            task_set.clear();
            for &c in system.params_to_constraints.get(&param)? {
                if let Some((task, outs)) = system.constraints[c].choose_method(param) {
                    q.extend(outs);
                    task_set.insert(task);
                }
            }
            tasks.extend(&task_set);
        }
        if tasks.is_empty() {
            None
        } else {
            Some(EffectPath { tasks, effects })
        }
    }
}
