/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use core::panic;
use std::{
    borrow::Cow,
    collections::{
        hash_map::{DefaultHasher, Entry},
        HashMap, HashSet, VecDeque,
    },
    hash::{Hash, Hasher},
};

use slotmap::SlotMap;

use crate::{Task, TaskData, TaskDyn, TaskParameterDyn, VariableRegister, World, WorldNameAccess};

#[derive(Default)]
struct Register {
    ins: HashSet<TaskParameterDyn>,
    outs: HashSet<TaskParameterDyn>,
}

impl VariableRegister for Register {
    fn register_var(&mut self, ref_kind: crate::RefKindVariant, var: TaskParameterDyn) {
        match ref_kind {
            crate::RefKindVariant::Ref => self.ins.insert(var),
            crate::RefKindVariant::Mut => self.outs.insert(var),
        };
    }
}

#[derive(Clone)]
struct Method {
    task: TaskDyn,
    outputs: Vec<TaskParameterDyn>,
    inputs: Vec<TaskParameterDyn>,
}

#[derive(Clone)]
struct Variable {
    constraints: HashSet<ConstraintRef>,
    strength: i32,
    value: TaskParameterDyn,
}

#[derive(Clone)]
pub(crate) struct Constraint {
    name: Option<Cow<'static, str>>,
    variables: HashSet<TaskParameterDyn>,
    methods: Vec<Method>,
    selected_method: Option<usize>,
    last_input: Option<TaskParameterDyn>,
}

slotmap::new_key_type! {
    struct ConstraintRef;
}

#[derive(Default, Clone)]
pub struct System {
    vars: HashMap<TaskParameterDyn, Variable>,
    constraints: SlotMap<ConstraintRef, Constraint>,
}

pub struct ConstraintBuilder<'a> {
    constraint: Constraint,
    world: &'a World,
}

trait MethodChooser {
    fn choose_method(
        &self,
        input: TaskParameterDyn,
        strength_from: impl Fn(&TaskParameterDyn) -> i32 + Copy,
    ) -> usize;
}

impl<'a> ConstraintBuilder<'a> {
    pub(crate) fn new<T: TaskData>(task: Task<T>, world: &'a World) -> Self {
        let mut register = Register::default();
        world.register_vars(task, &mut register);
        if !register.ins.is_disjoint(&register.outs) {
            panic!("Input and output variables have to be disjoint");
        }

        let all_vars = register
            .ins
            .union(&register.outs)
            .copied()
            .collect::<HashSet<_>>();

        let method = Method {
            task: task.erase(),
            outputs: register.outs.into_iter().collect(),
            inputs: register.ins.into_iter().collect(),
        };
        Self {
            constraint: Constraint {
                name: None,
                variables: all_vars,
                methods: vec![method],
                selected_method: None,
                last_input: None,
            },
            world,
        }
    }

    pub fn add_method<T: TaskData>(mut self, task: Task<T>) -> Self {
        let mut register = Register::default();
        self.world.register_vars(task, &mut register);
        if !register.ins.is_disjoint(&register.outs) {
            panic!("Input and output variables have to be disjoint");
        }

        if self.constraint.variables
            != register
                .ins
                .union(&register.outs)
                .copied()
                .collect::<HashSet<_>>()
        {
            panic!("Method has different TaskParameters than the constraint.");
        }

        self.constraint.methods.push(Method {
            task: task.erase(),
            outputs: register.outs.into_iter().collect(),
            inputs: register.ins.into_iter().collect(),
        });
        self
    }

    pub fn name(mut self, n: impl Into<Cow<'static, str>>) -> Self {
        self.constraint.name = Some(n.into());
        self
    }
}

impl System {
    pub fn add_constraint(&mut self, mut constraint: ConstraintBuilder) {
        let is_simple = constraint.constraint.methods.len() == 1;
        if is_simple {
            constraint.constraint.selected_method = Some(0);
        }
        let constraint_ref = self.constraints.insert(constraint.constraint);
        let constraint = &self.constraints[constraint_ref];

        for &param in &constraint.variables {
            match self.vars.entry(param) {
                Entry::Occupied(mut o) => {
                    let var = o.get_mut();
                    var.constraints.insert(constraint_ref);
                    if !is_simple {
                        var.strength += 1;
                    }
                }
                Entry::Vacant(v) => {
                    v.insert(Variable {
                        constraints: HashSet::from([constraint_ref]),
                        strength: if is_simple { 0 } else { 1 },
                        value: param,
                    });
                }
            }
        }
    }

    pub fn write_graphvis(&self, world: &impl WorldNameAccess, out: impl Into<Cow<'static, str>>) {
        use graphviz_rust::cmd::{CommandArg, Format, Layout};
        use graphviz_rust::dot_generator::*;
        use graphviz_rust::dot_structures::*;
        use graphviz_rust::printer::PrinterContext;
        let mut dot = graph!(strict id!("ConstraintSystem"); attr!("overlap", "false"), attr!("ranksep", "1.5"), attr!("compound", "true"));

        for (c_idx, c) in self.constraints.iter() {
            let c_idx = c_idx.0.as_ffi();
            let cluster_id = format!("cluster_{c_idx}");

            let mut sub = Vec::new();
            let middle_point_idx = c.methods.len() / 2;
            for (t_idx, task) in c.methods.iter().enumerate() {
                let id = if t_idx == middle_point_idx {
                    format!("t_{c_idx}_point")
                } else {
                    format!("t_{c_idx}_{t_idx}")
                };
                let name = world
                    .task_name_dyn(task.task)
                    .as_ref()
                    .map(|n| format!("\"{n} ({t_idx})\""))
                    .unwrap_or_else(|| format!("\"<Unknown> ({t_idx})\""));
                sub.push(stmt!(
                    node!(id; attr!("shape", "hexagon"), attr!("label", name))
                ));
            }

            if let Some(name) = c.name.as_ref().map(|n| format!("\"{n}\"")) {
                sub.push(stmt!(attr!("label", name)));
            }
            let sub = Subgraph {
                id: id!(cluster_id),
                stmts: sub,
            };
            dot.add_stmt(stmt!(sub));
        }

        for (v_idx, var) in self.vars.iter() {
            let style = match var.value {
                TaskParameterDyn::Variable(_) => attr!("color", "black"),
                TaskParameterDyn::Event(_) => attr!("color", "green"),
                TaskParameterDyn::Effect(_) => attr!("color", "red"),
                TaskParameterDyn::Resource(_) => attr!("color", "blue"),
            };
            let mut hasher = DefaultHasher::new();
            v_idx.hash(&mut hasher);
            let id = format!("v_{}", hasher.finish());
            let name = world
                .task_parameter_name_dyn(var.value)
                .as_ref()
                .map(|n| format!("\"{n} ({})\"", var.strength))
                .unwrap_or_else(|| format!("\"<Unknown> ({})\"", var.strength));
            dot.add_stmt(stmt!(
                node!(id; attr!("shape", "oval"), style, attr!("label", name))
            ));

            for cn in &var.constraints {
                let constraint = &self.constraints[*cn];
                let c_idx = cn.0.as_ffi();
                let middle_point_idx = constraint.methods.len() / 2;
                match constraint.selected_method {
                    Some(t_idx) => {
                        let point_id = if t_idx == middle_point_idx {
                            format!("t_{c_idx}_point")
                        } else {
                            format!("t_{c_idx}_{t_idx}")
                        };
                        let stroke_attr = match constraint.last_input {
                            Some(p) if p == *v_idx => attr!("style", "bold"),
                            _ => attr!("style", "normal"),
                        };
                        let edge = if constraint.methods[t_idx].outputs.contains(&var.value) {
                            edge!(node_id!(point_id) => node_id!(id); attr!("dir", "forward"), attr!("arrowhead", "normal"), stroke_attr)
                        } else {
                            edge!(node_id!(id) => node_id!(point_id); attr!("dir", "forward"), attr!("arrowhead", "normal"), stroke_attr)
                        };
                        dot.add_stmt(stmt!(edge));
                    }
                    None => {
                        let point_id = format!("t_{c_idx}_point");
                        let cluster_id = format!("cluster_{c_idx}");
                        dot.add_stmt(stmt!(
                            edge!(node_id!(id) => node_id!(point_id); attr!("lhead", cluster_id))
                        ));
                    }
                }
            }
        }

        let mut ctx = PrinterContext::default();
        let dot_str = graphviz_rust::print(dot, &mut ctx);
        let _empty = graphviz_rust::exec_dot(
            dot_str,
            vec![
                CommandArg::Format(Format::Svg),
                CommandArg::Output(out.into().into_owned()),
                CommandArg::Layout(Layout::Dot),
            ],
        )
        .unwrap();
    }

    pub fn plan_with_event(&mut self, event: impl Into<TaskParameterDyn>) {
        let event = event.into();
        let mut q = VecDeque::new();
        q.push_back(event);
        let mut visited_params = HashSet::new();
        let mut visited_constraints = HashSet::new();
        while let Some(var) = q.pop_front() {
            for c_ref in self.vars[&var]
                .constraints
                .clone()
                .into_iter()
                .filter(|&c_ref| visited_constraints.insert(c_ref))
            {
                let constraint = &mut self.constraints[c_ref];
                if constraint.methods.len() == 1 {
                    debug_assert!(constraint.selected_method.is_some());
                    for &out in &constraint.methods[0].outputs {
                        if visited_params.insert(out) {
                            q.push_back(out);
                        }
                    }
                    continue;
                }
                let selected_m = constraint
                    .methods
                    .choose_method(var, |p| self.vars[p].strength);

                // reset from previous selected method
                if let Some(previous_selected_method_idx) = constraint.selected_method {
                    let method = &constraint.methods[previous_selected_method_idx];
                    for input in method
                        .inputs
                        .iter()
                        .filter(|&&param| Some(param) != constraint.last_input)
                    {
                        self.vars.get_mut(input).unwrap().strength += 1;
                    }
                    for output in method.outputs.iter() {
                        self.vars.get_mut(output).unwrap().strength -= 1;
                    }
                }

                let method = &constraint.methods[selected_m];

                for input in method.inputs.iter().filter(|&&param| param != var) {
                    self.vars.get_mut(input).unwrap().strength -= 1;
                }
                for output in &method.outputs {
                    self.vars.get_mut(output).unwrap().strength += 1;
                    if visited_params.insert(*output) {
                        q.push_back(*output);
                    }
                }
                constraint.last_input = Some(var);
                constraint.selected_method = Some(selected_m);
            }
        }
    }
}

impl MethodChooser for Vec<Method> {
    fn choose_method(
        &self,
        input: TaskParameterDyn,
        strength_from: impl Fn(&TaskParameterDyn) -> i32 + Copy,
    ) -> usize {
        self.iter()
            .enumerate()
            .filter(|&(_, m)| m.inputs.contains(&input))
            .map(|(idx, m)| {
                (
                    idx,
                    m.inputs.iter().map(strength_from).sum::<i32>()
                        - m.outputs.iter().map(strength_from).sum::<i32>(),
                )
            })
            .max_by_key(|(_, score)| *score)
            .map(|(m, _)| m)
            .unwrap()
    }
}
