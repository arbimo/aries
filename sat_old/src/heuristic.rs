use crate::all::BVar;
use aries_collections::heap::IdxHeap;
use aries_collections::Next;

pub struct HeurParams {
    var_inc: f64,
    var_decay: f64,
}
impl Default for HeurParams {
    fn default() -> Self {
        HeurParams {
            var_inc: 1_f64,
            var_decay: 0.95_f64,
        }
    }
}

/// Heuristic value associated to a variable.
#[derive(Copy, Clone, PartialEq, PartialOrd)]
struct HVal {
    activity: f64,
}

impl Default for HVal {
    fn default() -> Self {
        HVal { activity: 1_f64 }
    }
}

pub struct Heur {
    params: HeurParams,
    heap: IdxHeap<BVar, HVal>,
}

impl Heur {
    pub fn init(num_vars: u32, params: HeurParams) -> Self {
        let mut h = Heur {
            params,
            heap: IdxHeap::with_elements(num_vars as usize, HVal::default()),
        };
        for v in BVar::first(num_vars as usize) {
            h.heap.enqueue(v);
        }
        h
    }

    /// Declares a new variable. The variable is NOT added to the queue.
    pub fn declare_variable(&mut self, v: BVar) {
        // TODO: what's the default value if the search is already ongoing
        self.heap.declare_element(v, HVal::default());
    }

    pub fn pop_next_var(&mut self) -> Option<BVar> {
        self.heap.pop()
    }

    pub fn peek_next_var(&mut self) -> Option<BVar> {
        self.heap.peek().copied()
    }

    pub fn var_insert(&mut self, var: BVar) {
        self.heap.enqueue(var)
    }

    pub fn var_bump_activity(&mut self, var: BVar) {
        let var_inc = self.params.var_inc;
        self.heap.change_priority(var, |p| p.activity += var_inc);
        if self.heap.priority(var).activity > 1e100_f64 {
            self.var_rescale_activity()
        }
    }

    pub fn decay_activities(&mut self) {
        self.params.var_inc /= self.params.var_decay;
    }

    fn var_rescale_activity(&mut self) {
        // here we scale the activity of all variables
        // this can not change the relative order in the heap, since activities are scaled by the same amount.
        self.heap.change_all_priorities_in_place(|p| p.activity *= 1e-100_f64);

        self.params.var_inc *= 1e-100_f64;
    }
}
