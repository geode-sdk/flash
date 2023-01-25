
use std::{sync::Arc, fs, collections::HashMap, path::PathBuf, ffi::OsStr};
use crate::{html::{Html, HtmlText, HtmlElement}, url::UrlPath, config::Config};

use super::{
    builder::{BuildResult, Builder, Entry, NavItem, OutputEntry},
    shared::{fmt_markdown, extract_title_from_md, fmt_section},
};

pub struct Tutorial {
    path: UrlPath,
    title: String,
    unparsed_content: String,
}

impl<'e> Entry<'e> for Tutorial {
    fn name(&self) -> String {
        self.path.raw_file_name().unwrap().replace(".md", "")
    }

    fn url(&self) -> UrlPath {
        UrlPath::parse("tutorials").unwrap().join(&self.path)
    }

    fn build(&self, builder: &Builder<'e>) -> BuildResult {
        builder.create_output_for(self)
    }

    fn nav(&self) -> NavItem {
        NavItem::new_link(&self.title, self.url(), None)
    }
}

impl<'e> OutputEntry<'e> for Tutorial {
    fn output(&self, builder: &Builder<'e>) -> (Arc<String>, Vec<(&'static str, Html)>) {
        (
            builder.config.templates.tutorial.clone(),
            vec![
                ("title", HtmlText::new(self.name()).into()),
                ("content", fmt_markdown(&self.unparsed_content)),
            ]
        )
    }
}

impl<'e> Tutorial {
    pub fn new(config: Arc<Config>, path: UrlPath) -> Self {
        let unparsed_content = fs::read_to_string(
            config.input_dir
                .join(&config.tutorials.as_ref().unwrap().dir)
                .join(&path.to_pathbuf())
        ).expect(&format!("Unable to read tutorial {}", path.to_raw_string()));

        Self {
            title: extract_title_from_md(&unparsed_content)
                .unwrap_or(path.raw_file_name().unwrap()),
            unparsed_content,
            path
        }
    }
}

pub struct TutorialFolder {
    is_root: bool,
    path: UrlPath,
    index: Option<String>,
    pub folders: HashMap<String, TutorialFolder>,
    pub tutorials: HashMap<String, Tutorial>,
}

impl<'e> Entry<'e> for TutorialFolder {
    fn name(&self) -> String {
        self.path.raw_file_name().unwrap_or(String::from("_"))
    }

    fn url(&self) -> UrlPath {
        if self.is_root {
            UrlPath::new()
        }
        else {
            UrlPath::parse("tutorials").unwrap().join(&self.path)
        }
    }

    fn build(&self, builder: &Builder<'e>) -> BuildResult {
        let mut handles = Vec::new();
        handles.extend(builder.create_output_for(self)?);
        for dir in self.folders.values() {
            handles.extend(dir.build(builder)?);
        }
        for file in self.tutorials.values() {
            handles.extend(file.build(builder)?);
        }
        Ok(handles)
    }

    fn nav(&self) -> NavItem {
        if self.is_root {
            NavItem::new_root(
                None,
                self.folders
                    .iter()
                    .map(|e| e.1.nav())
                    .chain(self.tutorials.iter().map(|e| e.1.nav()))
                    .collect::<Vec<_>>()
            )
        }
        else {
            NavItem::new_dir_open(
                &self.name(),
                self.folders
                    .iter()
                    .map(|e| e.1.nav())
                    .chain(self.tutorials.iter().map(|e| e.1.nav()))
                    .collect::<Vec<_>>(),
                None,
            )
        }
    }
}

impl<'e> TutorialFolder {
    fn from_folder(config: Arc<Config>, path: &PathBuf) -> Option<Self> {
        let mut folders = HashMap::new();
        let mut tutorials = HashMap::new();

        let stripped_path = path.strip_prefix(
            &config.input_dir.join(&config.tutorials.as_ref().unwrap().dir)
        ).unwrap_or(&path).to_path_buf();

        // find tutorials (markdown files)
        for file in fs::read_dir(path).ok()? {
            let Ok(file) = file else { continue; };
            let Ok(ty) = file.file_type() else { continue; };
            let path = file.path();

            // if this is a directory, add it only if it has tutorials
            if ty.is_dir() {
                if let Some(folder) = TutorialFolder::from_folder(
                    config.clone(), &file.path()
                ) {
                    folders.insert(folder.name(), folder);
                }
            }
            // markdown files are tutorials
            else if ty.is_file() && 
                path.extension() == Some(OsStr::new("md")) &&
                // skip special files
                match path.file_name().map(|f| f.to_string_lossy().to_lowercase()) {
                    Some(val) => match val.as_str() {
                        "readme.md" | "index.md" => false,
                        _ => true,
                    },
                    None => false,
                }
            {
                let stripped_path = path.strip_prefix(
                    &config.input_dir.join(&config.tutorials.as_ref().unwrap().dir)
                ).unwrap_or(&path).to_path_buf();
                
                let Ok(url) = UrlPath::try_from(&stripped_path) else { continue; };
                let tut = Tutorial::new(config.clone(), url);
                tutorials.insert(tut.name(), tut);
            }
        }

        // only consider this a tutorial folder if it has some tutorials
        (folders.len() > 0 || tutorials.len() > 0).then_some(Self {
            is_root: false,
            path: UrlPath::try_from(&stripped_path).ok()?,
            index: if path.join("index.md").exists() {
                fs::read_to_string(path.join("index.md")).ok()
            } else {
                None
            },
            folders,
            tutorials
        })
    }

    pub fn from_config(config: Arc<Config>) -> Self {
        if let Some(ref tutorials) = config.tutorials &&
            let Some(mut res) = Self::from_folder(
                config.clone(), &config.input_dir.join(&tutorials.dir)
            )
        {
            res.is_root = true;
            res
        }
        else {
            Self {
                is_root: true,
                path: UrlPath::new(),
                index: None,
                folders: HashMap::new(),
                tutorials: HashMap::new(),
            }
        }
    }
}

impl<'e> OutputEntry<'e> for TutorialFolder {
    fn output(&self, builder: &Builder<'e>) -> (Arc<String>, Vec<(&'static str, Html)>) {
        self.index.as_ref()
            .map(|index| (
                builder.config.templates.tutorial.clone(),
                vec![
                    ("title", HtmlText::new(self.name()).into()),
                    ("content", fmt_markdown(index)),
                ]
            ))
            .unwrap_or((
                builder.config.templates.tutorial_index.clone(),
                vec![
                    ("title", HtmlText::new(self.name()).into()),
                    ("links", fmt_section("Pages", self.tutorials.iter()
                        .map(|(_, tut)|
                            HtmlElement::new("ul")
                            .with_child(
                                HtmlElement::new("a")
                                .with_text(&tut.title)
                                .with_attr("href", tut.url().to_absolute(builder.config.clone()))
                            )
                            .into()
                        )
                        .collect()
                    )),
                ]
            ))
    }
}