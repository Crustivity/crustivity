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
        BinaryHeap, HashMap, HashSet,
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
enum Methods {
    Real(Vec<Method>),
    Synthetic(TaskParameterDyn),
}

impl Methods {
    fn real(method: Method) -> Self {
        Self::Real(vec![method])
    }

    fn push_real(&mut self, method: Method) {
        if let Methods::Real(m) = self {
            m.push(method);
        }
    }

    fn len(&self) -> usize {
        match self {
            Methods::Real(v) => v.len(),
            Methods::Synthetic(_) => 1,
        }
    }

    fn outputs_at(&self, idx: usize) -> &[TaskParameterDyn] {
        match self {
            Methods::Real(v) => &v[idx].outputs,
            Methods::Synthetic(s) => std::slice::from_ref(s),
        }
    }

    fn find_outs(
        &self,
        mut predicate: impl FnMut(&[TaskParameterDyn]) -> Option<u32>,
    ) -> Option<(usize, &[TaskParameterDyn])> {
        match self {
            Methods::Real(v) => {
                if v.len() == 1 {
                    Some((0, v[0].outputs.as_slice()))
                } else {
                    v.iter()
                        .enumerate()
                        .filter_map(|(i, m)| predicate(&m.outputs).map(|strength| (i, m, strength)))
                        .map(|(idx, outs, strength)| (idx, outs.outputs.as_slice(), strength))
                        .max_by_key(|(_, _, strength)| *strength)
                        .map(|(idx, outs, _)| (idx, outs))
                }
            }

            Methods::Synthetic(s) => {
                predicate(std::slice::from_ref(s)).map(|_| (0, std::slice::from_ref(s)))
            }
        }
    }
}

#[derive(Clone)]
struct Variable {
    determined_by: Option<ConstraintRef>,
    constraints: Vec<ConstraintRef>,
    num_constraints: usize,
    mark: bool,
    value: TaskParameterDyn,
    strength: Option<u32>,
}

#[derive(Clone)]
pub(crate) struct Constraint {
    name: Option<Cow<'static, str>>,
    variables: HashSet<TaskParameterDyn>,
    methods: Methods,
    selected_method: Option<usize>,
    strength: u32,
    mark: bool,
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

pub struct Planner {
    system: System,
    unsatisfied_cns: HashSet<ConstraintRef>,
    free_variables: Vec<TaskParameterDyn>,
    unenforced_cns: BinaryHeap<ConstraintRef>,
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
                methods: Methods::real(method),
                selected_method: None,
                strength: u32::MAX,
                mark: false,
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

        self.constraint.methods.push_real(Method {
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
    pub fn add_stay_constraint(&mut self, param: TaskParameterDyn) {
        let constraint = Constraint {
            name: Some(Cow::Borrowed("Strong stay")),
            variables: {
                let mut s = HashSet::new();
                s.insert(param);
                s
            },
            methods: Methods::Synthetic(param),
            selected_method: None,
            strength: 0,
            mark: false,
        };
        let c_ref = self.constraints.insert(constraint);
        match self.vars.entry(param) {
            Entry::Occupied(mut o) => {
                let var = o.get_mut();
                if !var.constraints.contains(&c_ref) {
                    var.constraints.push(c_ref);
                    var.num_constraints += 1;
                }
            }
            Entry::Vacant(v) => {
                v.insert(Variable {
                    determined_by: None,
                    constraints: vec![c_ref],
                    num_constraints: 1,
                    strength: None,
                    mark: false,
                    value: param,
                });
            }
        }
    }

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
                    if !var.constraints.contains(&constraint_ref) {
                        var.constraints.push(constraint_ref);
                        if !is_simple {
                            var.num_constraints += 1;
                        }
                    }
                }
                Entry::Vacant(v) => {
                    v.insert(Variable {
                        determined_by: None,
                        constraints: vec![constraint_ref],
                        num_constraints: if is_simple { 0 } else { 1 },
                        strength: None,
                        mark: false,
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
            match &c.methods {
                Methods::Real(m) => {
                    for (t_idx, task) in m.iter().enumerate() {
                        let id = if t_idx == middle_point_idx {
                            format!("t_{c_idx}_point")
                        } else {
                            format!("t_{c_idx}_{t_idx}")
                        };
                        if let Some(name) = world
                            .task_name_dyn(task.task)
                            .as_ref()
                            .map(|n| format!("\"{n}\""))
                        {
                            sub.push(stmt!(
                                node!(id; attr!("shape", "hexagon"), attr!("label", name))
                            ));
                        } else {
                            sub.push(stmt!(node!(id; attr!("shape", "hexagon"))));
                        }
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
                Methods::Synthetic(s) => {
                    let id = format!("t_{c_idx}_point");
                    dot.add_stmt(stmt!(
                        node!(id; attr!("label", "\"strong stay\""), attr!("shape", "box"))
                    ));
                }
            }
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
            if let Some(name) = world
                .task_parameter_name_dyn(var.value)
                .as_ref()
                .map(|n| format!("\"{n}\""))
            {
                dot.add_stmt(stmt!(
                    node!(id; attr!("shape", "oval"), style, attr!("label", name))
                ));
            } else {
                dot.add_stmt(stmt!(node!(id; attr!("shape", "oval"), style)));
            }

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
                        let edge = if constraint.methods.outputs_at(t_idx).contains(&var.value) {
                            edge!(node_id!(point_id) => node_id!(id); attr!("dir", "forward"), attr!("arrowhead", "normal"))
                        } else {
                            edge!(node_id!(id) => node_id!(point_id); attr!("dir", "forward"), attr!("arrowhead", "normal"))
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
        println!("{dot_str}");
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

    pub fn set_strength(&mut self, strength: u32, param: TaskParameterDyn) {
        if let Some(v) = self.vars.get_mut(&param) {
            v.strength = Some(strength);
        }
    }
}
