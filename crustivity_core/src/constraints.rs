/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use core::panic;
use std::collections::{HashMap, HashSet, VecDeque};

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

pub struct Effect {
    task: TaskDyn,
    params: HashSet<TaskParameterDyn>,
}

impl Effect {
    pub fn new<T: TaskParam>(task: Task<T>, world: &World) -> Self {
        let mut register = Register::default();
        task.register_vars(&mut register, world);
        if !register.1.is_empty() {
            panic!("Effect cannot have out parameters");
        }
        Self {
            task: task.erase(),
            params: register.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ConstraintTaskIndex {
    Constraint(usize),
    Effect(usize),
}

pub struct ConstraintSystem {
    constraints: Vec<Constraint>,
    effects: Vec<TaskDyn>,
    params_to_constraints_or_effect: HashMap<TaskParameterDyn, Vec<ConstraintTaskIndex>>,
}

impl ConstraintSystem {
    pub(crate) fn new() -> Self {
        Self {
            constraints: Vec::new(),
            effects: Vec::new(),
            params_to_constraints_or_effect: HashMap::new(),
        }
    }

    pub fn add_constraint(&mut self, constraint: Constraint) {
        let c_idx = self.constraints.len();
        for param in constraint.params.iter().copied() {
            self.params_to_constraints_or_effect
                .entry(param)
                .or_insert_with(Vec::new)
                .push(ConstraintTaskIndex::Constraint(c_idx));
        }
        self.constraints.push(constraint);
    }

    pub fn add_effect(&mut self, effect: Effect) {
        let e_idx = self.effects.len();
        for param in effect.params {
            self.params_to_constraints_or_effect
                .entry(param)
                .or_insert_with(Vec::new)
                .push(ConstraintTaskIndex::Effect(e_idx));
        }
        self.effects.push(effect.task);
    }
}

pub(crate) struct EffectPath {
    pub(crate) tasks: Vec<TaskDyn>,
    pub(crate) effects: Vec<TaskDyn>,
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
        let mut effect_path = EffectPath {
            tasks: Vec::new(),
            effects: Vec::new(),
        };

        let mut q = VecDeque::<TaskParameterDyn>::new();
        q.push_back(param);

        let mut tasks = HashSet::new();
        let mut effects = HashSet::new();
        while let Some(param) = q.pop_front() {
            tasks.clear();
            for &constrain_task_idx in system.params_to_constraints_or_effect.get(&param)? {
                match constrain_task_idx {
                    ConstraintTaskIndex::Constraint(c) => {
                        if let Some((task, outs)) = system.constraints[c].choose_method(param) {
                            q.extend(outs);
                            tasks.insert(task);
                        }
                    }
                    ConstraintTaskIndex::Effect(e) => {
                        effects.insert(system.effects[e]);
                    }
                }
            }
            effect_path.tasks.extend(&tasks);
        }
        effect_path.effects.extend(effects.into_iter());
        if effect_path.tasks.is_empty() && effect_path.effects.is_empty() {
            None
        } else {
            Some(effect_path)
        }
    }
}
