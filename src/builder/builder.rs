use clang::{Entity, EntityKind};
use indicatif::ProgressBar;
use std::{collections::HashMap, fs};
use strfmt::strfmt;

use crate::config::Config;

use super::{namespace::Namespace, files::Root, index::Index};

pub enum NavItem {
    Root(Option<String>, Vec<NavItem>),
    Dir(String, Vec<NavItem>, Option<String>),
    Link(String, String, Option<String>),
}

impl NavItem {
    pub fn new_link(name: &str, url: &str, icon: Option<&str>) -> NavItem {
        NavItem::Link(name.into(), url.into(), icon.map(|s| s.into()))
    }

    pub fn new_dir(name: &str, items: Vec<NavItem>, icon: Option<&str>) -> NavItem {
        NavItem::Dir(name.into(), items, icon.map(|s| s.into()))
    }

    pub fn new_root(name: Option<&str>, items: Vec<NavItem>) -> NavItem {
        NavItem::Root(name.map(|s| s.into()), items)
    }

    pub fn to_html(&self, nest_level: usize) -> String {
        match self {
            NavItem::Link(name, url, icon) => format!(
                "<a href='{}'>{}{}</a>",
                // If we're in a nested folder already, we first have to 
                // navigate back to root
                relative_url(url, nest_level),
                icon
                    .as_ref()
                    .map(|i| format!("<i data-feather='{}' class='icon'></i>", i))
                    .unwrap_or(String::new()),
                name
            ),

            NavItem::Dir(name, items, icon) => format!(
                "<details>
                    <summary>
                        <i data-feather='chevron-right'></i>
                        {}{}
                    </summary>
                    <div>{}</div>
                </details>",
                icon
                    .as_ref()
                    .map(|i| format!("<i data-feather='{}' class='icon'></i>", i))
                    .unwrap_or(String::new()),
                name,
                items.iter().map(|i| i.to_html(nest_level)).collect::<String>()
            ),

            NavItem::Root(name, items) => if let Some(name) = name {
                format!(
                    "<details open class='root'>
                        <summary>
                            <i data-feather='chevron-right'></i>
                            {}
                        </summary>
                        <div>{}</div>
                    </details>",
                    name, items.iter().map(|i| i.to_html(nest_level)).collect::<String>()
                )
            } else {
                items.iter().map(|i| i.to_html(nest_level)).collect::<String>()
            }
        }
    }
}

pub trait AnEntry<'e> {
    fn name(&self) -> String;
    fn url(&self) -> String;
    fn build(&self, builder: &Builder<'_, 'e>) -> Result<(), String>;
    fn nav(&self) -> NavItem;
}

pub trait OutputEntry<'c, 'e>: AnEntry<'e> {
    fn output(&self, builder: &Builder<'c, 'e>) -> (&'c String, Vec<(String, String)>);
}

pub struct Builder<'c, 'e> {
    pub config: &'c Config,
    root: Namespace<'e>,
    file_roots: Vec<Root<'c>>,
    nav_caches: HashMap<usize, String>,
}

impl<'c, 'e> Builder<'c, 'e> {
    pub fn new(config: &'c Config, root: Entity<'e>) -> Self {
        Self {
            config,
            root: Namespace::new(root),
            file_roots: Root::from_config(config),
            nav_caches: HashMap::new(),
        }.setup()
    }

    fn setup(self) -> Self {
        for script in self.config.presentation.css.iter().chain(&self.config.presentation.js) {
            fs::write(
                self.config.output_dir.join(&script.name),
                &script.content
            ).unwrap();
        }
        self
    }

    pub fn create_output_for<E: OutputEntry<'c, 'e>>(&self, entry: &E) -> Result<(), String> {
        let (template, vars) = entry.output(self);
        let target_url = &entry.url();
        let nest_level = get_nest_level(target_url);
        
        let mut fmt = default_format(self.config, nest_level);
        fmt.extend(vars);
    
        let content = strfmt(&template, &fmt)
            .map_err(|e| format!("Unable to format {target_url}: {e}"))?;
        
        let page = strfmt(
            &self.config.presentation.page_template,
            &HashMap::from([
                (
                    "head_content".to_owned(), 
                    strfmt(
                        &self.config.presentation.head_template,
                        &default_format(self.config, nest_level)
                    ).map_err(|e| format!("Unable to format head for {target_url}: {e}"))?
                ),
                ("navbar_content".to_owned(), self.build_nav(nest_level)?),
                ("main_content".to_owned(), content),
            ])
        )
            .map_err(|e| format!("Unable to format {target_url}: {e}"))?;

        fs::create_dir_all(self.config.output_dir.join(target_url))
            .map_err(|e| format!("Unable to create directory for {target_url}: {e}"))?;

        fs::write(&self.config.output_dir.join(target_url).join("index.html"), page)
            .map_err(|e| format!("Unable to save {target_url}: {e}"))?;
    
        Ok(())
    }

    fn all_entries(&self) -> Vec<&dyn AnEntry<'e>> {
        self.root.entries
            .iter()
            .map(|p| p.1 as &dyn AnEntry<'e>)
            .chain(self.file_roots.iter().map(|p| p as &dyn AnEntry<'e>))
            .collect()
    }

    pub fn build(&mut self, pbar: Option<&ProgressBar>) -> Result<(), String> {
        // For tracking progress
        let entries_len = self.root.entries.len();
        let total_len = (entries_len + self.file_roots.len()) as f64;

        // Prebuild cached navbars for much faster docs builds
        self.prebuild_navs()?;

        // Create docs for all entries
        let mut i = 0f64;
        for entry in self.all_entries() {
            if let Some(pbar) = pbar {
                pbar.set_position((i / total_len * pbar.length().unwrap_or(1) as f64) as u64);
            }
            i += 1f64;
            entry.build(self)?;
        }

        // Create root index.html
        self.create_output_for(&Index {})?;
    
        Ok(())
    }

    pub fn build_nav(&self, nest_level: usize) -> Result<String, String> {
        if let Some(cached) = self.nav_caches.get(&nest_level) {
            return Ok(cached.to_owned());
        }

        let mut fmt = default_format(self.config, nest_level);
        fmt.extend([
            ("entity_content".into(), self.root.nav().to_html(nest_level)),
            (
                "file_content".into(),
                self.file_roots
                    .iter()
                    .map(|root| root.nav().to_html(nest_level))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        ]);
        Ok(
            strfmt(&self.config.presentation.nav_template, &fmt)
                .map_err(|e| format!("Unable to format navbar: {e}"))?
        )
    }

    fn prebuild_navs(&mut self) -> Result<(), String> {
        for lvl in self
            .all_entries()
            .iter()
            .map(|p| get_nest_level(&p.url()))
            .collect::<Vec<_>>()
        {
            if !self.nav_caches.contains_key(&lvl) {
                self.nav_caches.insert(lvl, self.build_nav(lvl)?);
            }
        }
        Ok(())
    }
}

pub fn get_fully_qualified_name(entity: &Entity) -> Vec<String> {
    let mut name = Vec::new();
    if let Some(parent) = entity.get_semantic_parent() {
        if !matches!(parent.get_kind(), EntityKind::TranslationUnit) {
            name.extend(get_fully_qualified_name(&parent));
        }
    }
    name.push(entity.get_name().unwrap_or("_anon".into()));
    name
}

pub fn get_header_url(config: &Config, entity: &Entity) -> Option<String> {
    let path = entity
        .get_definition()?
        .get_location()?
        .get_file_location()
        .file?
        .get_path();

    Some(
        config.docs.tree.clone()?
            + "/"
            + &path
                .strip_prefix(&config.input_dir)
                .unwrap_or(&path)
                .to_str()?
                .replace("\\", "/"),
    )
}

fn relative_url(url: &String, nest_level: usize) -> String {
    format!(".{}/{}", "/..".repeat(nest_level), url)
}

fn get_nest_level(url: &String) -> usize {
    url.matches("/").count()
}

fn default_format(config: &Config, nest_level: usize) -> HashMap<String, String> {
    HashMap::from([
        ("project_name".into(), config.project.name.clone()),
        ("project_version".into(), config.project.version.clone()),
        ("root_url".into(), format!(".{}", "/..".repeat(nest_level))),
    ])
}
