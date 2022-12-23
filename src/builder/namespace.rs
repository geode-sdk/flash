
use std::collections::HashMap;

use clang::{Entity, EntityKind};

use super::{builder::{Builder, AnEntry, get_fully_qualified_name, NavItem}, class::Class};

pub enum CppItem<'e> {
    Namespace(Namespace<'e>),
    Class(Class<'e>),
}

impl<'e> AnEntry<'e> for CppItem<'e> {
    fn name(&self) -> String {
        match self {
            CppItem::Namespace(ns) => ns.name(),
            CppItem::Class(cs) => cs.name(),
        }
    }
    
    fn url(&self) -> String {
        match self {
            CppItem::Namespace(ns) => ns.url(),
            CppItem::Class(cs) => cs.url(),
        }
    }

    fn build(&self, builder: &Builder<'_, 'e>) -> Result<(), String> {
        match self {
            CppItem::Namespace(ns) => ns.build(builder),
            CppItem::Class(cs) => cs.build(builder),
        }
    }

    fn nav(&self) -> NavItem {
        match self {
            CppItem::Namespace(ns) => ns.nav(),
            CppItem::Class(cs) => cs.nav(),
        }
    }
}

pub struct Namespace<'e> {
    entity: Entity<'e>,
    pub entries: HashMap<String, CppItem<'e>>,
}

impl<'e> AnEntry<'e> for Namespace<'e> {
    fn build(&self, builder: &Builder<'_, 'e>) -> Result<(), String> {
        for (_, entry) in &self.entries {
            entry.build(builder)?;
        }
        Ok(())
    }

    fn nav(&self) -> NavItem {
        let mut entries = self.entries.iter().collect::<Vec<_>>();

        // Namespaces first in sorted order, everything else after in sorted order
        entries.sort_by_key(|p| (!matches!(p.1, CppItem::Namespace(_)), p.0));

        if self.entity.get_kind() == EntityKind::TranslationUnit {
            NavItem::new_root(None, entries.iter().map(|e| e.1.nav()).collect())
        }
        else {
            NavItem::new_dir(
                &self.name(),
                entries
                    .iter()
                    .map(|e| e.1.nav())
                    .collect(),
                None
            )
        }
    }

    fn name(&self) -> String {
        self.entity.get_name().unwrap_or("<Anonymous namespace>".into())
    }

    fn url(&self) -> String {
        String::from("./") + &get_fully_qualified_name(&self.entity).join("/")
    }
}

impl<'e> Namespace<'e> {
    pub fn new(entity: Entity<'e>) -> Self {
        let mut ret = Self {
            entity,
            entries: HashMap::new(),
        };
        ret.load_entries();
        ret
    }

    fn load_entries(&mut self) {
        for child in &self.entity.get_children() {
            if child.is_in_system_header() || child.get_name().is_none() {
                continue;
            }
            match child.get_kind() {
                EntityKind::Namespace => {
                    let entry = Namespace::new(child.clone());
                    // Merge existing entries of namespace
                    if let Some(key) = self.entries.get_mut(&entry.name()) {
                        if let CppItem::Namespace(ns) = key {
                            ns.entries.extend(entry.entries);
                        }
                    }
                    // Insert new namespace
                    else {
                        self.entries.insert(entry.name(), CppItem::Namespace(entry));
                    }
                },

                EntityKind::StructDecl | EntityKind::ClassDecl => {
                    if child.is_definition() {
                        let entry = Class::new(child.clone());
                        self.entries.insert(entry.name(), CppItem::Class(entry));
                    }
                },

                _ => continue,
            }
        }
    }
}