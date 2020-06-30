use crate::cesta::Event::{EdgeActivated, EdgeAdded, NewPendingActivation, NodeAdded};
use crate::FloatLike;
use std::collections::{HashSet, VecDeque};

type Node = u32;
type Edge = u32;

#[derive(Copy, Clone, Debug)]
struct Constraint<W> {
    /// True if the constraint appear in explanations
    internal: bool,
    /// True if the constraint active (participates in propagation)
    active: bool,
    source: Node,
    target: Node,
    weight: W,
}

type BacktrackLevel = u32;

enum Event<W> {
    Level(BacktrackLevel),
    NodeAdded,
    EdgeAdded,
    NewPendingActivation,
    EdgeActivated(Edge),
    ForwardUpdate {
        node: Node,
        previous_dist: W,
        previous_cause: Option<Edge>,
    },
    BackwardUpdate {
        node: Node,
        previous_dist: W,
        previous_cause: Option<Edge>,
    },
}

#[derive(Ord, PartialOrd, PartialEq, Eq, Debug)]
pub enum NetworkStatus<'a> {
    /// Network is fully propagated and consistent
    Consistent,
    /// Network is inconsistent, due to the presence of the given negative cycle.
    /// Note that internal edges (typically those inserted to represent lower/upper bounds) are
    /// omitted from the inconsistent set.
    Inconsistent(&'a [Edge]),
}

struct Distance<W> {
    forward: W,
    forward_cause: Option<Edge>,
    forward_pending_update: bool,
    backward: W,
    backward_cause: Option<Edge>,
    backward_pending_update: bool,
}

impl<W: FloatLike> Distance<W> {
    pub fn new(lb: W, ub: W) -> Self {
        Distance {
            forward: ub,
            forward_cause: None,
            forward_pending_update: false,
            backward: -lb,
            backward_cause: None,
            backward_pending_update: false,
        }
    }
}

/// STN that supports
///  - incremental edge addition and consistency checking with [Cesta96]
///  - undoing the latest changes
///  - providing explanation on inconsistency in the form of a culprit
///         set of constraints
///
/// Once the network reaches an inconsistent state, the only valid operation
/// is to undo the latest change go back to a consistent network. All other
/// operations have an undefined behavior.
pub struct IncSTN<W> {
    constraints: Vec<Constraint<W>>,
    /// Forward/Backward adjacency list containing active edges.
    active_forward_edges: Vec<Vec<Edge>>,
    active_backward_edges: Vec<Vec<Edge>>,
    distances: Vec<Distance<W>>,
    /// History of changes and made to the STN with all information necessary to undo them.
    trail: Vec<Event<W>>,
    pending_activations: VecDeque<Edge>,
    level: BacktrackLevel,
    /// Internal data structure to construct explanations as negative cycles.
    /// When encountering an inconsistency, this vector will be cleared and
    /// a negative cycle will be constructed in it. The explanation returned
    /// will be a slice of this vector to avoid any allocation.
    explanation: Vec<Edge>,
}

impl<W: FloatLike> IncSTN<W> {
    /// Creates a new STN. Initially, the STN contains a single timepoint
    /// representing the origin whose domain is [0,0]. The id of this timepoint can
    /// be retrieved with the `origin()` method.
    pub fn new() -> Self {
        let mut stn = IncSTN {
            constraints: vec![],
            active_forward_edges: vec![],
            active_backward_edges: vec![],
            distances: vec![],
            trail: vec![],
            pending_activations: VecDeque::new(),
            level: 0,
            explanation: vec![],
        };
        let origin = stn.add_node(W::zero(), W::zero());
        assert_eq!(origin, stn.origin());
        // make sure that initialization of the STN can not be undone
        stn.trail.clear();
        stn
    }
    pub fn num_nodes(&self) -> u32 {
        debug_assert_eq!(self.active_forward_edges.len(), self.active_backward_edges.len());
        self.active_forward_edges.len() as u32
    }

    pub fn num_edges(&self) -> u32 {
        self.constraints.len() as u32
    }

    pub fn origin(&self) -> Node {
        0
    }

    pub fn lb(&self, node: Node) -> W {
        -self.distances[node as usize].backward
    }
    pub fn ub(&self, node: Node) -> W {
        self.distances[node as usize].forward
    }

    /// Adds a new node to the STN with a domain of `[lb, ub]`.
    /// Returns the identifier of the newly added node.
    /// Lower and upper bounds have corresponding edges in the STN distance
    /// graph that will participate in propagation:
    ///  - `ORIGIN --(ub)--> node`
    ///  - `node --(-lb)--> ORIGIN`
    /// However, those edges are purely internal and since their IDs are not
    /// communicated, they will be omitted when appearing in the explanation of
    /// inconsistencies.  
    /// If you want for those to appear in explanation, consider setting bounds
    /// to -/+ infinity adding those edges manually.
    ///
    /// Panics if `lb > ub`. This guarantees that the network remains consistent
    /// when adding a node.
    ///
    pub fn add_node(&mut self, lb: W, ub: W) -> Node {
        assert!(lb <= ub);
        let id = self.num_nodes();
        self.active_forward_edges.push(Vec::new());
        self.active_backward_edges.push(Vec::new());
        self.trail.push(NodeAdded);
        let fwd_edge = self.add_constraint(Constraint {
            internal: true,
            active: false,
            source: self.origin(),
            target: id,
            weight: ub,
        });
        let bwd_edge = self.add_constraint(Constraint {
            internal: true,
            active: false,
            source: id,
            target: self.origin(),
            weight: -lb,
        });
        // todo: these should not require propagation because they will properly set the
        //       node's domain. However mark_active will add them to the propagation queue
        self.mark_active(fwd_edge);
        self.mark_active(bwd_edge);
        self.distances.push(Distance {
            forward: ub,
            forward_cause: Some(fwd_edge),
            forward_pending_update: false,
            backward: -lb,
            backward_cause: Some(bwd_edge),
            backward_pending_update: false,
        });
        id
    }

    pub fn add_edge(&mut self, source: Node, target: Node, weight: W) -> Edge {
        let id = self.add_inactive_edge(source, target, weight);
        self.mark_active(id);
        id
    }

    /// Records an INACTIVE new edge and returns its identifier.
    /// After calling this method, the edge is inactive and will not participate in
    /// propagation. The edge can be activated with the `mark_active()` method.
    ///
    /// Since the edge is inactive, the STN remains consistent after calling this method.
    pub fn add_inactive_edge(&mut self, source: Node, target: Node, weight: W) -> Edge {
        let c = Constraint {
            internal: false,
            active: false,
            source,
            target,
            weight,
        };
        self.add_constraint(c)
    }

    /// Marks an edge as active. No changes are commited to the network by this function
    /// until a call to `propagate_all()`
    pub fn mark_active(&mut self, edge: Edge) {
        self.pending_activations.push_back(edge);
        self.trail.push(Event::NewPendingActivation);
    }

    /// Propagates all edges that have been marked as active since the last propagation.
    pub fn propagate_all(&mut self) -> NetworkStatus {
        while let Some(edge) = self.pending_activations.pop_front() {
            let c = &mut self.constraints[edge as usize];
            if c.source == c.target {
                if c.weight < W::zero() {
                    // negative self loop: inconsistency
                    self.explanation.clear();
                    self.explanation.push(edge);
                    return NetworkStatus::Inconsistent(&self.explanation);
                } else {
                    // positive self loop : useless edge that we can ignore
                }
            } else if !c.active {
                c.active = true;
                self.active_forward_edges[c.source as usize].push(edge);
                self.active_backward_edges[c.target as usize].push(edge);
                self.trail.push(EdgeActivated(edge));
                // if self.propagate(edge) != NetworkStatus::Consistent;
                if let NetworkStatus::Inconsistent(explanation) = self.propagate(edge) {
                    // work around borrow checker, transmutation should be a no-op that just resets lifetimes
                    let x = unsafe { std::mem::transmute(explanation) };
                    return NetworkStatus::Inconsistent(x);
                }
            }
        }
        NetworkStatus::Consistent
    }

    pub fn set_backtrack_point(&mut self) -> BacktrackLevel {
        self.level += 1;
        self.trail.push(Event::Level(self.level));
        self.level
    }

    pub fn undo_to_last_backtrack_point(&mut self) -> Option<BacktrackLevel> {
        while let Some(ev) = self.trail.pop() {
            match ev {
                Event::Level(lvl) => return Some(lvl),
                NodeAdded => {
                    self.active_forward_edges.pop();
                    self.active_backward_edges.pop();
                    self.distances.pop();
                }
                EdgeAdded => {
                    self.constraints.pop();
                }
                NewPendingActivation => {
                    self.pending_activations.pop_back();
                }
                EdgeActivated(e) => {
                    let c = &mut self.constraints[e as usize];
                    self.active_forward_edges[c.source as usize].pop();
                    self.active_backward_edges[c.target as usize].pop();
                    c.active = false;
                }
                Event::ForwardUpdate {
                    node,
                    previous_dist,
                    previous_cause,
                } => {
                    let x = &mut self.distances[node as usize];
                    x.forward = previous_dist;
                    x.forward_cause = previous_cause;
                }
                Event::BackwardUpdate {
                    node,
                    previous_dist,
                    previous_cause,
                } => {
                    let x = &mut self.distances[node as usize];
                    x.backward = previous_dist;
                    x.backward_cause = previous_cause;
                }
            }
        }
        None
    }

    fn add_constraint(&mut self, c: Constraint<W>) -> Edge {
        assert!(
            c.source < self.num_nodes() && c.target < self.num_nodes(),
            "Unrecorded node"
        );
        let id = self.num_edges();
        self.constraints.push(c);
        self.trail.push(EdgeAdded);
        id
    }

    fn fdist(&self, n: Node) -> W {
        self.distances[n as usize].forward
    }
    fn bdist(&self, n: Node) -> W {
        self.distances[n as usize].backward
    }
    fn weight(&self, e: Edge) -> W {
        self.constraints[e as usize].weight
    }
    fn active(&self, e: Edge) -> bool {
        self.constraints[e as usize].active
    }
    fn source(&self, e: Edge) -> Node {
        self.constraints[e as usize].source
    }
    fn target(&self, e: Edge) -> Node {
        self.constraints[e as usize].target
    }

    /// Implementation of [Cesta96]
    fn propagate(&mut self, edge: Edge) -> NetworkStatus {
        let mut queue = VecDeque::new();
        // fast access to check if a node is in the queue
        // this can be improve with a bitset, and might not be necessary since
        // any work is guarded by the pending update flags
        let mut in_queue = HashSet::new();
        let c = &self.constraints[edge as usize];
        debug_assert_ne!(c.source, c.target, "This algorithm does not support self loops.");
        let i = c.source;
        let j = c.target;
        queue.push_back(i);
        in_queue.insert(i);
        queue.push_back(j);
        in_queue.insert(j);
        self.distances[i as usize].forward_pending_update = true;
        self.distances[i as usize].backward_pending_update = true;
        self.distances[j as usize].forward_pending_update = true;
        self.distances[j as usize].backward_pending_update = true;

        while let Some(u) = queue.pop_front() {
            in_queue.remove(&u);
            if self.distances[u as usize].forward_pending_update {
                for &out_edge in &self.active_forward_edges[u as usize] {
                    // TODO(perf): we should avoid touching the constraints array by adding target and weight to forward edges
                    let c = &self.constraints[out_edge as usize];
                    debug_assert!(self.active(out_edge));
                    debug_assert_eq!(u, c.source);
                    let previous = self.fdist(c.target);
                    let candidate = self.fdist(c.source) + c.weight;
                    if candidate < previous {
                        if candidate + self.bdist(c.target) < W::zero() {
                            return NetworkStatus::Inconsistent(self.extract_cycle_backward(out_edge));
                        }
                        self.trail.push(Event::ForwardUpdate {
                            node: c.target,
                            previous_dist: previous,
                            previous_cause: self.distances[c.target as usize].forward_cause,
                        });
                        self.distances[c.target as usize].forward = candidate;
                        self.distances[c.target as usize].forward_cause = Some(out_edge);
                        self.distances[c.target as usize].forward_pending_update = true;
                        if !in_queue.contains(&c.target) {
                            queue.push_back(c.target);
                            in_queue.insert(c.target);
                        }
                    }
                }
            }

            if self.distances[u as usize].backward_pending_update {
                for &in_edge in &self.active_backward_edges[u as usize] {
                    let c = &self.constraints[in_edge as usize];
                    debug_assert!(self.active(in_edge));
                    debug_assert_eq!(u, c.target);
                    let previous = self.bdist(c.source);
                    let candidate = self.bdist(c.target) + c.weight;
                    if candidate < previous {
                        if candidate + self.fdist(c.source) < W::zero() {
                            return NetworkStatus::Inconsistent(self.extract_cycle_forward(in_edge));
                        }
                        self.trail.push(Event::BackwardUpdate {
                            node: c.source,
                            previous_dist: previous,
                            previous_cause: self.distances[c.source as usize].backward_cause,
                        });
                        self.distances[c.source as usize].backward = candidate;
                        self.distances[c.source as usize].backward_cause = Some(in_edge);
                        self.distances[c.source as usize].backward_pending_update = true;
                        if !in_queue.contains(&c.source) {
                            queue.push_back(c.source);
                            in_queue.insert(c.source);
                        }
                    }
                }
            }
            // problematic in the case of self cycles...
            self.distances[u as usize].forward_pending_update = false;
            self.distances[u as usize].backward_pending_update = false;
        }
        NetworkStatus::Consistent
    }

    /// Builds a cycle by going back up the backward causes until a cycle is found.
    /// Returns a set of active non-internal edges that are part in a negative cycle
    /// involving `edge`.
    /// Panics if no such cycle exists.
    fn extract_cycle_backward(&mut self, edge: Edge) -> &[Edge] {
        self.explanation.clear();
        self.explanation.push(edge);
        let e = &self.constraints[edge as usize];
        let source = e.source;
        let target = e.target;
        let mut current = target;
        loop {
            let next_constraint_id = self.distances[current as usize]
                .backward_cause
                .expect("No cause on member of cycle");
            let nc = &self.constraints[next_constraint_id as usize];
            if !nc.internal {
                self.explanation.push(next_constraint_id);
            }
            current = nc.target;
            if current == source {
                return &self.explanation;
            } else if current == self.origin() {
                break;
            }
        }
        debug_assert_eq!(current, self.origin());
        current = source;
        loop {
            let next_constraint_id = self.distances[current as usize]
                .forward_cause
                .expect("No cause on member of cycle");

            let nc = &self.constraints[next_constraint_id as usize];
            if !nc.internal {
                self.explanation.push(next_constraint_id);
            }
            current = nc.source;
            if current == self.origin() {
                return &self.explanation;
            }
            debug_assert_ne!(current, source);
        }
    }

    /// Builds a cycle by going back up the forward causes until a cycle is found.
    /// Returns a set of active non-internal edges that are part in a negative cycle
    /// involving `edge`.
    /// Panics if no such cycle exists.
    fn extract_cycle_forward(&mut self, edge: Edge) -> &[Edge] {
        self.explanation.clear();
        self.explanation.push(edge);
        let e = &self.constraints[edge as usize];
        let source = e.source;
        let target = e.target;
        let mut current = source;
        loop {
            let next_constraint_id = self.distances[current as usize]
                .forward_cause
                .expect("No cause on member of cycle");

            let nc = &self.constraints[next_constraint_id as usize];
            if !nc.internal {
                self.explanation.push(next_constraint_id);
            }
            current = nc.source;
            if current == target {
                // we closed the loop, return the cycle
                return &self.explanation;
            } else if current == self.origin() {
                // met the origin, we should stop here, and finish building the loop
                // by going in the other direction from the target node.
                break;
            }
        }
        debug_assert_eq!(current, self.origin());
        current = target;
        loop {
            let next_constraint_id = self.distances[current as usize]
                .backward_cause
                .expect("No cause on member of cycle");
            let nc = &self.constraints[next_constraint_id as usize];
            if !nc.internal {
                self.explanation.push(next_constraint_id);
            }
            current = nc.target;
            if current == self.origin() {
                return &self.explanation;
            }
            debug_assert_ne!(
                current, source,
                "met the source edge while expecting to find the network's origin"
            );
        }
    }

    fn print(&self) {
        println!("Nodes: ");
        for (id, n) in self.distances.iter().enumerate() {
            println!(
                "{} [{}, {}] back_cause: {:?}  forw_cause: {:?}",
                id, -n.backward, n.forward, n.backward_cause, n.forward_cause
            );
        }
        println!("Active Edges:");
        for (id, &c) in self.constraints.iter().enumerate().filter(|x| x.1.active) {
            println!("{}: {} -- {} --> {} ", id, c.source, c.weight, c.target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cesta::NetworkStatus::{Consistent, Inconsistent};

    fn assert_consistent<W: FloatLike>(stn: &mut IncSTN<W>) {
        assert_eq!(stn.propagate_all(), Consistent);
    }
    fn assert_inconsistent<W: FloatLike>(stn: &mut IncSTN<W>, mut cycle: Vec<Edge>) {
        cycle.sort();
        match stn.propagate_all() {
            Consistent => panic!("Expected inconsistent network"),
            Inconsistent(exp) => {
                let mut vec: Vec<Edge> = exp.iter().copied().collect();
                vec.sort();
                assert_eq!(vec, cycle);
            }
        }
    }

    #[test]
    fn test_backtracking() {
        let mut stn = IncSTN::new();
        let a = stn.add_node(0, 10);
        let b = stn.add_node(0, 10);
        assert_eq!(stn.lb(a), 0);
        assert_eq!(stn.ub(a), 10);
        assert_eq!(stn.lb(b), 0);
        assert_eq!(stn.ub(b), 10);

        stn.add_edge(stn.origin(), a, 1);
        assert_consistent(&mut stn);
        assert_eq!(stn.lb(a), 0);
        assert_eq!(stn.ub(a), 1);
        assert_eq!(stn.lb(b), 0);
        assert_eq!(stn.ub(b), 10);
        stn.set_backtrack_point();

        let ab = stn.add_edge(a, b, 5i32);
        assert_consistent(&mut stn);
        assert_eq!(stn.lb(a), 0);
        assert_eq!(stn.ub(a), 1);
        assert_eq!(stn.lb(b), 0);
        assert_eq!(stn.ub(b), 6);

        stn.set_backtrack_point();

        let ba = stn.add_edge(b, a, -6i32);
        assert_inconsistent(&mut stn, vec![ab, ba]);

        stn.undo_to_last_backtrack_point();
        assert_eq!(stn.lb(a), 0);
        assert_eq!(stn.ub(a), 1);
        assert_eq!(stn.lb(b), 0);
        assert_eq!(stn.ub(b), 6);

        stn.undo_to_last_backtrack_point();
        assert_eq!(stn.lb(a), 0);
        assert_eq!(stn.ub(a), 1);
        assert_eq!(stn.lb(b), 0);
        assert_eq!(stn.ub(b), 10);

        let x = stn.add_inactive_edge(a, b, 5i32);
        stn.mark_active(x);
        assert_eq!(stn.propagate_all(), Consistent);
        assert_eq!(stn.lb(a), 0);
        assert_eq!(stn.ub(a), 1);
        assert_eq!(stn.lb(b), 0);
        assert_eq!(stn.ub(b), 6);
    }

    #[test]
    fn test_explanation() {
        let mut stn = IncSTN::new();
        let a = stn.add_node(0, 10);
        let b = stn.add_node(0, 10);
        let c = stn.add_node(0, 10);

        stn.set_backtrack_point();
        let aa = stn.add_inactive_edge(a, a, -1);
        stn.mark_active(aa);
        assert_inconsistent(&mut stn, vec![aa]);

        stn.undo_to_last_backtrack_point();
        stn.set_backtrack_point();
        let ab = stn.add_edge(a, b, 2);
        let ba = stn.add_edge(b, a, -3);
        assert_inconsistent(&mut stn, vec![ab, ba]);

        stn.undo_to_last_backtrack_point();
        stn.set_backtrack_point();
        let ab = stn.add_edge(a, b, 2);
        let _ = stn.add_edge(b, a, -2);
        assert_consistent(&mut stn);
        let ba = stn.add_edge(b, a, -3);
        assert_inconsistent(&mut stn, vec![ab, ba]);

        stn.undo_to_last_backtrack_point();
        stn.set_backtrack_point();
        let ab = stn.add_edge(a, b, 2);
        let bc = stn.add_edge(b, c, 2);
        let _ = stn.add_edge(c, a, -4);
        assert_consistent(&mut stn);
        let ca = stn.add_edge(c, a, -5);
        assert_inconsistent(&mut stn, vec![ab, bc, ca]);
    }
}
