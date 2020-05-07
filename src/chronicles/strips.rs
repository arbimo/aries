use crate::collection::id_map::IdMap;
use std::collections::HashMap;
use crate::chronicles::typesystem::{TypeHierarchy, TypeId};
use std::hash::Hash;
use std::fmt::{Display, Debug, Formatter, Error};
use streaming_iterator::StreamingIterator;
use crate::chronicles::state::{StateDesc, Lit};
use crate::chronicles::ddl::Arg;

// todo:
#[derive(Clone)]
pub struct SymbolTable<T, Sym> {
    pub types: TypeHierarchy<T>,
    symbols: Vec<Sym>,
    ids: HashMap<Sym, SymId>,
    instances_by_exact_type: IdMap<TypeId, Instances>
}

impl<T, Sym : Debug> Debug for SymbolTable<T, Sym> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        for (i, x) in self.symbols.iter().enumerate() {
            write!(f, "{:?}\t<- {:?}\n", SymId::from(i), x)?;
        }
        Ok(())
    }
}

#[derive(Copy,Clone,Debug)]
pub struct Instances {
    first: usize,
    after_last: usize
}

impl Instances {
    pub fn new(first: SymId, last: SymId) -> Self {
        let last_id: usize = last.into();
        Instances {
            first: first.into(),
            after_last: last_id + 1
        }
    }

    pub fn singleton(item: SymId) -> Self {
        Instances::new(item, item)
    }
}

impl Iterator for Instances {
    type Item = SymId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.first < self.after_last {
            self.first += 1;
            Some(SymId::from(self.first-1))
        } else {
            None
        }
    }
}

impl<T,Sym> SymbolTable<T,Sym>
{
    pub fn new(th: TypeHierarchy<T>, symbols: Vec<(Sym,T)>) -> Result<Self, String> where
        T: Clone + Eq + Hash,
        Sym: Clone + Eq + Hash + Display
    {

        let mut instances_by_type = HashMap::new();
        for (sym, tpe) in symbols {
            let tpe_id = th.id_of(&tpe).unwrap();
            instances_by_type
                .entry(tpe_id)
                .or_insert_with(|| Vec::with_capacity(1))
                .push(sym);
        }

        let mut table = SymbolTable {
            types: th,
            symbols: Default::default(),
            ids: Default::default(),
            instances_by_exact_type: Default::default()
        };

        for tpe in table.types.types() {
            let first = table.symbols.len();

            for sym in instances_by_type.remove(&tpe).unwrap_or(Vec::new()) {
                if table.ids.contains_key(&sym) {
                    return Result::Err(format!("duplicated instance : {}", sym));
                }
                let id = SymId::from(table.symbols.len());
                table.symbols.push(sym.clone());
                table.ids.insert(sym, id);
            }
            let after_last = table.symbols.len();
            table.instances_by_exact_type.insert(tpe, Instances { first, after_last });
        }

        Result::Ok(table)
    }

    pub fn id(&self, sym: &Sym) -> Option<SymId> where
    Sym : Eq + Hash
    {
        self.ids.get(sym).copied()
    }

    pub fn symbol(&self, id: SymId) -> &Sym {
        let i : usize = id.into();
        &self.symbols[i]
    }

    /// Returns an iterator on all direct or indirect instances of the given type
    pub fn instances_of_type(&self, tpe: TypeId) -> Instances
        where T : Clone + Eq + Hash
    {
        let mut instance = self.instances_by_exact_type[tpe];
        instance.after_last = self.instances_by_exact_type[self.types.last_subtype(tpe)].after_last;
        instance
    }
}


#[derive(Copy, Clone,Ord, PartialOrd, Eq, PartialEq, Debug, Hash)]
pub struct SymId(u32);

impl Into<usize> for SymId {
    fn into(self) -> usize {
        self.0 as usize
    }
}

impl From<usize> for SymId {
    fn from(i: usize) -> Self {
        SymId(i as u32)
    }
}

struct Pred(Vec<SymId>);

#[derive(Copy,Clone,Debug)]
pub  enum ParamOrSym {
    Sym(SymId),
    Param(u32)
}
#[derive(Debug)]
pub struct ParameterizedPred {
    pub positive: bool,
    pub sexpr: Vec<ParamOrSym>
}



impl ParameterizedPred {

    pub fn bind<T,S>(&self, sd: &StateDesc<T,S>, params: &[SymId], working: &mut Vec<SymId>) -> Option<Lit> {
        working.clear();
        for &x in &self.sexpr {
            let sym = match x {
                ParamOrSym::Param(i) => params[i as usize],
                ParamOrSym::Sym(s) => s
            };
            working.push(sym);
        }
        sd.sv_id(working.as_slice())
            .map(|sv| Lit::new(sv, self.positive))

    }
}

#[derive(Copy, Clone,Ord, PartialOrd, Eq, PartialEq)]
struct PredId(u32);

impl Into<usize> for PredId {
    fn into(self) -> usize {
        self.0 as usize
    }
}

impl From<usize> for PredId {
    fn from(i: usize) -> Self {
        PredId(i as u32)
    }
}

pub struct ActionTemplate {
    pub name: String,
    pub params: Vec<Arg>,
    pub pre: Vec<ParameterizedPred>,
    pub eff: Vec<ParameterizedPred>,
}

struct Operator {
    id: usize,
    pre: Vec<PredId>,
    add: Vec<PredId>,
    del: Vec<PredId>
}


#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::chronicles::enumerate::enumerate;


    #[test]
    fn instances() {
        let symbols = table();
        let types = &symbols.types;
        let rover = types.id_of(&"rover").unwrap();
        let predicate = types.id_of(&"predicate").unwrap();
        let location = types.id_of(&"location").unwrap();
        let object = types.id_of(&"object").unwrap();
        assert_eq!(symbols.instances_of_type(predicate).count(), 2);
        assert_eq!(symbols.instances_of_type(object).count(), 3);
        assert_eq!(symbols.instances_of_type(rover).count(), 1);
        assert_eq!(symbols.instances_of_type(location).count(), 2);
    }

    #[test]
    fn enumeration() {
        let symbols = table();
        let types = &symbols.types;
        let rover = types.id_of(&"rover").unwrap();
        let predicate = types.id_of(&"predicate").unwrap();
        let location = types.id_of(&"location").unwrap();
        let _object = types.id_of(&"object").unwrap();
        let x = [
            symbols.instances_of_type(predicate),
            symbols.instances_of_type(rover),
            symbols.instances_of_type(location),
            symbols.instances_of_type(location)
        ];

        let mut xx = enumerate(x.to_vec());

        while let Some(comb) = xx.next() {
            println!("{:?}", comb)
        }
        println!("DONE")

    }


    pub fn table() -> SymbolTable<&'static str,&'static str> {
        let types = vec![
            ("predicate", None),
            ("object", None),
            ("rover", Some("object")),
            ("location", Some("object"))
        ];
        let types = TypeHierarchy::new(types).unwrap();

        let instances = vec![
            ("at", "predicate"),
            ("can_traverse", "predicate"),
            ("rover1", "rover"),
            ("l1", "location"),
            ("l2", "location")
        ];
        let symbols = SymbolTable::new(types.clone(), instances).unwrap();
        symbols
    }
}