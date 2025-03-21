use clang::{Clang, Entity};
use indicatif::ProgressBar;
use serde_json::json;
use std::{collections::HashMap, sync::Arc};
use strfmt::strfmt;
use tokio::task::JoinHandle;

use crate::{
    config::Config,
    html::{
        process::{minify_css, minify_html, minify_js},
        GenHtml, Html,
    },
    url::UrlPath,
};

use super::{
    files::Root,
    namespace::Namespace,
    traits::{BuildResult, Entry, OutputEntry},
    tutorial::TutorialFolder,
};

pub struct Builder<'e> {
    pub config: Arc<Config>,
    pub root: Namespace<'e>,
    pub _clang: &'e Clang,
    pub index: &'e clang::Index<'e>,
    pub args: &'e [String],
    file_roots: Vec<Root>,
    tutorials: TutorialFolder,
    nav_cache: Option<String>,
}

impl<'e> Builder<'e> {
    pub fn new(
        config: Arc<Config>,
        root: Entity<'e>,
        clang: &'e Clang,
        index: &'e clang::Index<'e>,
        args: &'e [String],
    ) -> Result<Self, String> {
        Self {
            config: config.clone(),
            root: Namespace::new_root(root, config.clone()),
            _clang: clang,
            index,
            args,
            file_roots: Root::from_config(config.clone()),
            tutorials: TutorialFolder::from_config(config),
            nav_cache: None,
        }
        .setup()
    }

    fn setup(mut self) -> Result<Self, String> {
        // copy & minify CSS
        for script in &self.config.scripts.css {
            std::fs::write(
                self.config.output_dir.join(&script.name),
                minify_css(script.content.to_string())?,
            )
            .map_err(|e| format!("Unable to copy {}: {e}", script.name))?;
        }

        // transpile, minify, and copy JS
        for script in &self.config.scripts.js {
            std::fs::write(
                self.config.output_dir.join(&script.name),
                minify_js(script.content.to_string())?,
            )
            .map_err(|e| format!("Unable to copy {}: {e}", script.name))?;
        }

        // copy icon
        if let Some(ref icon) = self.config.project.icon {
            std::fs::copy(
                self.config.input_dir.join(icon),
                self.config.output_dir.join("icon.png"),
            )
            .map_err(|e| format!("Unable to copy icon: {e}"))?;

            let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
            let ico = ico::IconImage::read_png(
                std::fs::File::open(self.config.input_dir.join(icon)).unwrap(),
            )
            .map_err(|e| format!("Icon doesn't appear to be a valid .png: {e}"))?;
            icon_dir.add_entry(ico::IconDirEntry::encode(&ico).unwrap());
            let ico_file =
                std::fs::File::create(self.config.output_dir.join("favicon.ico")).unwrap();
            icon_dir.write(ico_file).unwrap();
        }

        // copy tutorial assets
        if let Some(ref tutorials) = self.config.tutorials {
            for asset in &tutorials.assets {
                let output = self.config.output_dir.join(
                    // if the tutorials are in docs and the assets are in
                    // docs/assets, then they are probably referenced with
                    // just assets/image.png so we should strip the docs
                    // part
                    asset.strip_prefix(&tutorials.dir).unwrap_or(asset),
                );
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(self.config.output_dir.join(parent)).map_err(|e| {
                        format!(
                            "Unable to create asset directory '{}': {e}",
                            output.to_string_lossy()
                        )
                    })?;
                }
                std::fs::copy(self.config.input_dir.join(asset), output).map_err(|e| {
                    format!(
                        "Unable to copy asset '{}': {e}, {}",
                        asset.to_string_lossy(),
                        self.config.input_dir.join(asset).to_string_lossy(),
                    )
                })?;
            }
        }

        // prebuild nav for performance
        self.prebuild()?;

        Ok(self)
    }

    pub fn create_output_for<E: OutputEntry<'e>>(&'e self, entry: &E) -> BuildResult {
        let (template, vars) = entry.output(self);
        Ok(vec![Self::create_output_in_thread(
            self.config.clone(),
            self.build_nav()?,
            entry.name(),
            entry.description(self),
            entry.url(),
            template,
            vars,
        )])
    }

    fn create_output_in_thread(
        config: Arc<Config>,
        nav: String,
        name: String,
        description: String,
        target_url: UrlPath,
        template: Arc<String>,
        vars: Vec<(&'static str, Html)>,
    ) -> JoinHandle<Result<UrlPath, String>> {
        tokio::spawn(async move {
            let title = if name.is_empty() {
                format!("{} Docs", config.project.name)
            } else {
                format!("{} - {} Docs", name, config.project.name)
            };

            let mut fmt = default_format(config.clone());
            fmt.extend(HashMap::from([
                (
                    "page_url".to_owned(),
                    target_url.to_absolute(config.clone()).to_string(),
                ),
                ("page_title".to_owned(), title.clone()),
                ("page_description".to_owned(), description.clone()),
            ]));
            fmt.extend(
                vars.into_iter()
                    .map(|(k, v)| (k.to_string(), v.gen_html()))
                    .collect::<Vec<_>>(),
            );

            let content = minify_html(
                strfmt(&template, &fmt)
                    .map_err(|e| format!("Unable to format {target_url}: {e}"))?,
            )?;

            let mut page = default_format(config.clone());
            page.extend(HashMap::from([
                (
                    "head_content".to_owned(),
                    strfmt(&config.templates.head, &fmt)
                        .map_err(|e| format!("Unable to format head for {target_url}: {e}"))?,
                ),
                ("navbar_content".to_owned(), nav),
                ("main_content".to_owned(), content.clone()),
            ]));
            let page = minify_html(
                strfmt(&config.templates.page, &page)
                    .map_err(|e| format!("Unable to format {target_url}: {e}"))?,
            )?;

            let output_dir = config.output_dir.join(target_url.to_pathbuf());

            // Make sure output directory exists
            tokio::fs::create_dir_all(&output_dir)
                .await
                .map_err(|e| format!("Unable to create directory for {target_url}: {e}"))?;

            // Save metadata to a file
            tokio::fs::write(
                output_dir.join("metadata.json"),
                format!(
                    r#"{{"title": "{}", "description": "{}"}}"#,
                    title, description,
                ),
            )
            .await
            .map_err(|e| format!("Unable to save metadata for {target_url}: {e}"))?;

            // Write the plain content output
            tokio::fs::write(
                config
                    .output_dir
                    .join(target_url.to_pathbuf())
                    .join("content.html"),
                content,
            )
            .await
            .map_err(|e| format!("Unable to save {target_url}: {e}"))?;

            // Write the full page
            tokio::fs::write(
                config
                    .output_dir
                    .join(target_url.to_pathbuf())
                    .join("index.html"),
                page,
            )
            .await
            .map_err(|e| format!("Unable to save {target_url}: {e}"))?;

            Ok(target_url)
        })
    }

    fn all_entries(&self) -> Vec<&dyn Entry<'e>> {
        self.root
            .entries
            .iter()
            .map(|p| p.1 as &dyn Entry<'e>)
            .chain(self.file_roots.iter().map(|p| p as &dyn Entry<'e>))
            .chain([&self.tutorials as &dyn Entry])
            .collect()
    }

    fn prebuild(&mut self) -> Result<(), String> {
        // Prebuild cached navbars for much faster docs builds
        self.prebuild_nav()?;

        Ok(())
    }

    pub async fn build(&self, pbar: Option<Arc<ProgressBar>>) -> Result<(), String> {
        let mut handles = Vec::new();

        // Spawn threads for creating docs for all entries
        for entry in self.all_entries() {
            handles.extend(entry.build(self)?);
        }

        if let Some(pbar) = pbar.clone() {
            pbar.set_message("Generating output".to_string());
        }

        futures::future::join_all(handles.into_iter().map(|handle| {
            let pbar = pbar.clone();
            tokio::spawn(async move {
                let res = handle.await.map_err(|e| format!("Unable to join {e}"))??;
                if let Some(pbar) = pbar {
                    pbar.set_message(format!("Built {res}"));
                }
                Result::<(), String>::Ok(())
            })
        }))
        .await
        .into_iter()
        .collect::<Result<Result<Vec<_>, _>, _>>()
        .map_err(|e| format!("Unable to join {e}"))??;

        if let Some(pbar) = pbar.clone() {
            pbar.set_message("Generating metadata".to_string());
        }

        tokio::fs::write(
            self.config.output_dir.join("functions.json"),
            serde_json::to_string(
                &self
                    .root
                    .nav()
                    .suboptions_titles(self.config.clone())
                    .into_iter()
                    .map(|(n, c)| {
                        if c > 0 {
                            format!("{} ({})", n, c + 1)
                        } else {
                            n
                        }
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| format!("Unable to save metadata {e}"))?,
        )
        .await
        .map_err(|e| format!("Unable to save metadata {e}"))?;

        tokio::fs::write(
            self.config.output_dir.join("nav.json"),
            serde_json::to_string(&self.build_nav_metadata()).unwrap(),
        )
        .await
        .unwrap();

        Ok(())
    }

    fn build_nav_metadata(&self) -> serde_json::Value {
        let tutorials = self.tutorials.nav().to_json(self.config.clone());
        let entities = self.root.nav().to_json(self.config.clone());
        json!({
            "tutorials": tutorials,
            "entities": entities,
        })
    }

    pub fn build_nav(&self) -> Result<String, String> {
        if let Some(ref cached) = self.nav_cache {
            return Ok(cached.to_owned());
        }
        let fmt = default_format(self.config.clone());
        strfmt(&self.config.templates.nav, &fmt)
            .map_err(|e| format!("Unable to format navbar: {e}"))
    }

    fn prebuild_nav(&mut self) -> Result<(), String> {
        self.nav_cache = Some(self.build_nav()?);
        Ok(())
    }
}

fn default_format(config: Arc<Config>) -> HashMap<String, String> {
    HashMap::from([
        ("project_name".into(), config.project.name.clone()),
        ("project_version".into(), config.project.version.clone()),
        (
            "project_repository".into(),
            config.project.repository.clone().unwrap_or_default(),
        ),
        (
            "project_icon".into(),
            config
                .project
                .icon
                .as_ref()
                .and(Some(format!(
                    "<img src=\"{}/icon.png\">",
                    config.output_url.as_ref().unwrap_or(&UrlPath::new())
                )))
                .unwrap_or_default(),
        ),
        (
            "output_url".into(),
            config
                .output_url
                .as_ref()
                .unwrap_or(&UrlPath::new())
                .to_string(),
        ),
    ])
}
