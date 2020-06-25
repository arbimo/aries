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
    Level(u32),
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
pub enum NetworkStatus {
    Consistent,
    Inconsistent,
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
///  - TODO undoing the latest changes
///  - TODO providing explanation on inconsistency in the form of a culprit
///         set of constraints
///
/// Once the network reaches an inconsistent state, the only valid operation
/// is to undo the latest change go back to a consistent network. All other
/// operations have an undefined behavior.
pub struct IncSTN<W> {
    constraints: Vec<Constraint<W>>,
    active_forward_edges: Vec<Vec<Edge>>,
    active_backward_edges: Vec<Vec<Edge>>,
    distances: Vec<Distance<W>>,
    history: Vec<Event<W>>,
    pending_activations: VecDeque<Edge>,
    level: BacktrackLevel,
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
            history: vec![],
            pending_activations: VecDeque::new(),
            level: 0,
        };
        let origin = stn.add_node(W::zero(), W::zero());
        assert_eq!(origin, stn.origin());
        // make sure that initialization of the STN can not be undone
        stn.history.clear();
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
        self.history.push(NodeAdded);
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
        // todo: these should not require propagation because will properly set the
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
        self.history.push(Event::NewPendingActivation);
    }

    /// Propagates all edges that have been marked as active since the last propagation.
    pub fn propagate_all(&mut self) -> NetworkStatus {
        while let Some(edge) = self.pending_activations.pop_front() {
            let c = &mut self.constraints[edge as usize];
            if !c.active {
                c.active = true;
                self.active_forward_edges[c.source as usize].push(edge);
                self.active_backward_edges[c.target as usize].push(edge);
                self.history.push(EdgeActivated(edge));
                let status = self.propagate(edge);
                if status != NetworkStatus::Consistent {
                    return status;
                }
            }
        }
        NetworkStatus::Consistent
    }

    pub fn set_backtrack_point(&mut self) -> BacktrackLevel {
        self.level += 1;
        self.history.push(Event::Level(self.level));
        self.level
    }

    pub fn undo_until_last_backtrack_point(&mut self) -> Option<BacktrackLevel> {
        while let Some(ev) = self.history.pop() {
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
        self.history.push(EdgeAdded);
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
                            return NetworkStatus::Inconsistent; // TODO: extract path
                        }
                        self.history.push(Event::ForwardUpdate {
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
                            return NetworkStatus::Inconsistent; // TODO: extract path
                        }
                        self.history.push(Event::BackwardUpdate {
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

            self.distances[u as usize].forward_pending_update = false;
            self.distances[u as usize].backward_pending_update = false;
        }
        NetworkStatus::Consistent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cesta::NetworkStatus::{Consistent, Inconsistent};

    #[test]
    fn test1() {
        let mut stn = IncSTN::new();
        let a = stn.add_node(0, 10);
        let b = stn.add_node(0, 10);
        assert_eq!(stn.lb(a), 0);
        assert_eq!(stn.ub(a), 10);
        assert_eq!(stn.lb(b), 0);
        assert_eq!(stn.ub(b), 10);

        let x = stn.add_inactive_edge(stn.origin(), a, 1);
        stn.mark_active(x);
        assert_eq!(stn.propagate_all(), Consistent);
        assert_eq!(stn.lb(a), 0);
        assert_eq!(stn.ub(a), 1);
        assert_eq!(stn.lb(b), 0);
        assert_eq!(stn.ub(b), 10);
        stn.set_backtrack_point();

        let x = stn.add_inactive_edge(a, b, 5i32);
        stn.mark_active(x);
        assert_eq!(stn.propagate_all(), Consistent);
        assert_eq!(stn.lb(a), 0);
        assert_eq!(stn.ub(a), 1);
        assert_eq!(stn.lb(b), 0);
        assert_eq!(stn.ub(b), 6);

        stn.set_backtrack_point();

        let x = stn.add_inactive_edge(b, a, -6i32);
        stn.mark_active(x);
        assert_eq!(stn.propagate_all(), Inconsistent);

        stn.undo_until_last_backtrack_point();
        assert_eq!(stn.lb(a), 0);
        assert_eq!(stn.ub(a), 1);
        assert_eq!(stn.lb(b), 0);
        assert_eq!(stn.ub(b), 6);

        stn.undo_until_last_backtrack_point();
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
}