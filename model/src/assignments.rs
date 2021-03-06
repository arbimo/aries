use crate::bounds::Bound;
use crate::int_model::{DiscreteModel, IntDomain};
use crate::lang::{Atom, BAtom, BExpr, IAtom, IVar, IntCst, SAtom, VarRef};
use crate::symbols::SymId;
use crate::symbols::{ContiguousSymbols, SymbolTable};
use crate::Model;

pub trait Assignment {
    fn symbols(&self) -> &SymbolTable;

    fn entails(&self, literal: Bound) -> bool;
    fn value_of_literal(&self, literal: Bound) -> Option<bool> {
        if self.entails(literal) {
            Some(true)
        } else if self.entails(!literal) {
            Some(false)
        } else {
            None
        }
    }
    fn is_undefined_literal(&self, literal: Bound) -> bool {
        self.value_of_literal(literal).is_none()
    }

    fn literal_of_expr(&self, expr: BExpr) -> Option<Bound>;

    fn var_domain(&self, var: impl Into<VarRef>) -> IntDomain;
    fn domain_of(&self, atom: impl Into<IAtom>) -> (IntCst, IntCst) {
        let atom = atom.into();
        let base = atom
            .var
            .map(|v| self.var_domain(v))
            .unwrap_or_else(|| IntDomain::new(0, 0));
        (base.lb + atom.shift, base.ub + atom.shift)
    }

    fn to_owned_assignment(&self) -> SavedAssignment;

    fn lower_bound(&self, int_var: IVar) -> IntCst {
        self.var_domain(int_var).lb
    }

    fn upper_bound(&self, int_var: IVar) -> IntCst {
        self.var_domain(int_var).ub
    }

    fn sym_domain_of(&self, atom: impl Into<SAtom>) -> ContiguousSymbols {
        let atom = atom.into();
        let (lb, ub) = self.int_bounds(atom);
        let lb = lb as usize;
        let ub = ub as usize;
        ContiguousSymbols::new(SymId::from(lb), SymId::from(ub))
    }

    fn sym_value_of(&self, atom: impl Into<SAtom>) -> Option<SymId> {
        self.sym_domain_of(atom).into_singleton()
    }

    /// Returns the value of a boolean atom if it as a set value.
    /// Return None otherwise meaning the value con be
    ///  - either true or false
    ///  - neither true nor false (empty domain)
    fn boolean_value_of(&self, batom: impl Into<BAtom>) -> Option<bool> {
        let batom = batom.into();
        match batom {
            BAtom::Cst(value) => Some(value),
            BAtom::Bound(b) => self.value_of_literal(b),
            BAtom::Expr(e) => self.literal_of_expr(e).and_then(|l| self.value_of_literal(l)),
        }
    }

    /// Return an integer view of the domain of any kind of atom.
    fn int_bounds(&self, atom: impl Into<Atom>) -> (IntCst, IntCst) {
        let atom = atom.into();
        match atom {
            Atom::Bool(atom) => match self.boolean_value_of(atom) {
                Some(true) => (1, 1),
                Some(false) => (0, 0),
                None => (0, 1),
            },
            Atom::Int(atom) => self.domain_of(atom),
            Atom::Sym(atom) => self.domain_of(atom.int_view()),
        }
    }
}

/// Extension trait that provides convenience methods to query the status of disjunctions.
pub trait DisjunctionExt<Disj>
where
    Disj: IntoIterator<Item = Bound>,
{
    fn entails(&self, literal: Bound) -> bool;
    fn value(&self, literal: Bound) -> Option<bool>;

    fn value_of_clause(&self, disjunction: Disj) -> Option<bool> {
        let mut found_undef = false;
        for disjunct in disjunction.into_iter() {
            match self.value(disjunct) {
                Some(true) => return Some(true),
                Some(false) => {}
                None => found_undef = true,
            }
        }
        if found_undef {
            None
        } else {
            Some(false)
        }
    }

    // =========== Clauses ============

    fn entailed_clause(&self, disjuncts: Disj) -> bool {
        self.value_of_clause(disjuncts) == Some(true)
    }
    fn violated_clause(&self, disjuncts: Disj) -> bool {
        self.value_of_clause(disjuncts) == Some(false)
    }
    fn pending_clause(&self, disjuncts: Disj) -> bool {
        let mut disjuncts = disjuncts.into_iter();
        while let Some(lit) = disjuncts.next() {
            if self.entails(lit) {
                return false;
            }
            if !self.entails(!lit) {
                // pending literal
                return disjuncts.all(|lit| !self.entails(lit));
            }
        }
        false
    }
    fn unit_clause(&self, disjuncts: Disj) -> bool {
        let mut disjuncts = disjuncts.into_iter();
        while let Some(lit) = disjuncts.next() {
            if self.entails(lit) {
                return false;
            }
            if !self.entails(!lit) {
                // pending literal, all others should be false
                return disjuncts.all(|lit| self.entails(!lit));
            }
        }
        // no pending literals founds, clause is not unit
        false
    }
}

impl<Disj: IntoIterator<Item = Bound>> DisjunctionExt<Disj> for DiscreteModel {
    fn entails(&self, literal: Bound) -> bool {
        self.entails(literal)
    }
    fn value(&self, literal: Bound) -> Option<bool> {
        self.value(literal)
    }
}

// TODO: this is correct but wasteful
pub type SavedAssignment = Model;

impl SavedAssignment {
    pub fn from_model(model: &Model) -> SavedAssignment {
        model.clone()
    }
}
