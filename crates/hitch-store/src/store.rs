//! Reading and saving the notes for a context.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Deserialize;

use crate::format::NotesFile;
use crate::locations::Locations;
use crate::project::{
    app_file_name, find_project, normalize, relative_to, site_file_name, start_folder,
};
use crate::{Context, NOTES_FILE_NAME, Note, StoreError, write_atomic};

/// Where notes and Hitch's own files live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreConfig {
    /// Name of the notes file in each project folder. `HITCH.md` unless
    /// the user changed it.
    pub file_name: String,
    /// Hitch's own folder, `~/.hitch`: the known-locations list and the
    /// search index live here.
    pub hitch_dir: PathBuf,
    /// Where notes for apps and web sites go: `apps/` and `sites/` inside
    /// it. `~/.hitch` unless the user changed it.
    pub fallback_dir: PathBuf,
}

/// The keys of `~/.hitch/settings.json` that the store reads. Other keys
/// belong to other parts of Hitch and are ignored here.
#[derive(Debug, Default, Deserialize)]
struct SettingsKeys {
    notes_file_name: Option<String>,
    fallback_dir: Option<PathBuf>,
}

impl StoreConfig {
    /// Defaults rooted at `hitch_dir`: `HITCH.md`, fallback files in
    /// `hitch_dir` itself.
    pub fn new(hitch_dir: impl Into<PathBuf>) -> Self {
        let hitch_dir = hitch_dir.into();
        Self {
            file_name: NOTES_FILE_NAME.to_owned(),
            fallback_dir: hitch_dir.clone(),
            hitch_dir,
        }
    }

    /// The user's config: `~/.hitch`, with `notes_file_name` and
    /// `fallback_dir` taken from `~/.hitch/settings.json` when set there.
    /// `None` when there is no home folder.
    pub fn for_user() -> Option<Self> {
        let dir = hitch_protocol::paths::hitch_dir()?;
        Some(Self::from_settings(dir))
    }

    /// Like [`StoreConfig::for_user`], for any Hitch folder.
    pub fn from_settings(hitch_dir: impl Into<PathBuf>) -> Self {
        let mut config = Self::new(hitch_dir);
        let keys = std::fs::read_to_string(config.settings_path())
            .ok()
            .and_then(|t| serde_json::from_str::<SettingsKeys>(&t).ok())
            .unwrap_or_default();
        if let Some(name) = keys.notes_file_name.filter(|n| valid_file_name(n)) {
            config.file_name = name;
        }
        if let Some(dir) = keys.fallback_dir {
            config.fallback_dir = expand_home(&dir);
        }
        config
    }

    /// `~/.hitch/settings.json`.
    pub fn settings_path(&self) -> PathBuf {
        self.hitch_dir.join("settings.json")
    }

    /// `~/.hitch/locations.json`.
    pub fn locations_path(&self) -> PathBuf {
        self.hitch_dir.join("locations.json")
    }

    /// The search index, `~/.hitch/index.sqlite`.
    pub fn index_path(&self) -> PathBuf {
        self.hitch_dir.join("index.sqlite")
    }

    /// Folder of per-app fallback files.
    pub fn apps_dir(&self) -> PathBuf {
        self.fallback_dir.join("apps")
    }

    /// Folder of per-site fallback files.
    pub fn sites_dir(&self) -> PathBuf {
        self.fallback_dir.join("sites")
    }
}

fn valid_file_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
}

fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

/// The notes of one notes file, as shown for one context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteGroup {
    /// The notes file. Pass it to [`Store::save_note`] and
    /// [`Store::delete_note`].
    pub path: PathBuf,
    /// The context the group was loaded for.
    pub context: Context,
    /// The notes, most relevant first: for a file context, the notes
    /// hitched to that file, then the rest in file order.
    pub notes: Vec<Note>,
}

/// Reads and saves notes. Cheap to share between threads.
#[derive(Debug)]
pub struct Store {
    config: StoreConfig,
    locations: Mutex<Option<Locations>>,
    /// Serialises read-modify-write cycles within this process.
    write_lock: Mutex<()>,
}

impl Store {
    pub fn new(config: StoreConfig) -> Self {
        Self {
            config,
            locations: Mutex::new(None),
            write_lock: Mutex::new(()),
        }
    }

    /// A store with the user's config (see [`StoreConfig::for_user`]).
    pub fn for_user() -> Option<Self> {
        StoreConfig::for_user().map(Self::new)
    }

    pub fn config(&self) -> &StoreConfig {
        &self.config
    }

    /// The configured notes file name.
    pub fn file_name(&self) -> &str {
        &self.config.file_name
    }

    // --- resolving paths ---------------------------------------------------

    /// The project folder for a path: [`find_project`], falling back to the
    /// path's own folder.
    pub fn project_for(&self, path: &Path) -> PathBuf {
        find_project(path, &self.config.file_name).unwrap_or_else(|| start_folder(path))
    }

    /// The context for a path on disk: a file inside its project, or the
    /// project of a folder.
    pub fn context_for_path(&self, path: &Path) -> Context {
        let path = normalize(path);
        let project = self.project_for(&path);
        if path.is_dir() {
            return Context::Folder { path: project };
        }
        match relative_to(&path, &project) {
            Some(file) if !file.as_os_str().is_empty() => Context::File { project, file },
            _ => Context::Folder { path: project },
        }
    }

    /// The notes file that holds a context's notes. It need not exist yet;
    /// saving a note creates it.
    pub fn notes_path(&self, context: &Context) -> PathBuf {
        match context {
            Context::File { project, .. } => project.join(&self.config.file_name),
            Context::Folder { path } => path.join(&self.config.file_name),
            Context::App { app_id } => self.config.apps_dir().join(app_file_name(app_id)),
            Context::Url { url } => self.config.sites_dir().join(site_file_name(url)),
        }
    }

    // --- reading -------------------------------------------------------------

    /// Read a notes file. A missing file reads as an empty one.
    pub fn load(&self, path: &Path) -> Result<NotesFile, StoreError> {
        match read_text(path)? {
            Some(text) => {
                self.remember(path);
                Ok(NotesFile::parse(&text))
            }
            None => Ok(NotesFile::new()),
        }
    }

    /// The notes for one context: one group, or none when its notes file
    /// is missing or empty. Doesn't walk up to parent folders; the context
    /// engine reports each parent's HITCH.md as a Folder context of its own.
    pub fn notes_for(&self, context: &Context) -> Result<Vec<NoteGroup>, StoreError> {
        let path = self.notes_path(context);
        let file = self.load(&path)?;
        if file.is_empty() {
            return Ok(Vec::new());
        }
        let mut notes: Vec<Note> = file.notes().cloned().collect();
        if let Context::File { file, .. } = context {
            // Stable: hitched notes first, each part in file order.
            notes.sort_by_key(|n| !n.is_hitched_to(file));
        }
        Ok(vec![NoteGroup {
            path,
            context: context.clone(),
            notes,
        }])
    }

    /// The notes for every active context, most specific first. A notes
    /// file shared by several contexts (a file and its project folder) is
    /// shown once, under the most specific.
    pub fn notes_for_all(&self, contexts: &[Context]) -> Result<Vec<NoteGroup>, StoreError> {
        let mut groups: Vec<NoteGroup> = Vec::new();
        for context in contexts {
            for group in self.notes_for(context)? {
                if !groups.iter().any(|g| g.path == group.path) {
                    groups.push(group);
                }
            }
        }
        Ok(groups)
    }

    // --- writing -------------------------------------------------------------

    /// Insert or update a note by id and return it as stored. A note with
    /// no id gets one, and a new note with no date gets today's. A note whose
    /// id isn't in the file (it was deleted elsewhere) is added rather than
    /// lost. Creates the notes file if needed.
    pub fn save_note(&self, path: &Path, note: Note) -> Result<Note, StoreError> {
        self.modify(path, |file| Ok(file.upsert(note).clone()))
    }

    /// Delete a note. `false` when there was no such note.
    pub fn delete_note(&self, path: &Path, id: &str) -> Result<bool, StoreError> {
        if !path.exists() {
            return Ok(false);
        }
        self.modify(path, |file| Ok(file.remove(id).is_some()))
    }

    /// Move a note to another notes file, keeping its id unless the target
    /// already uses it. The note is written to the target before it is
    /// removed from the source, so a failure never loses it.
    pub fn move_note(&self, from: &Path, id: &str, to: &Path) -> Result<Note, StoreError> {
        let note = self
            .load(from)?
            .get(id)
            .cloned()
            .ok_or_else(|| not_found(from, id))?;
        if normalize(from) == normalize(to) {
            return Ok(note);
        }
        let moved = self.modify(to, |file| {
            let mut note = note;
            if note.id.as_deref().is_some_and(|id| file.get(id).is_some()) {
                note.id = None;
            }
            Ok(file.upsert(note).clone())
        })?;
        self.delete_note(from, id)?;
        Ok(moved)
    }

    /// Re-hitch a note to another context. Within the same notes file this
    /// only changes the note's files (a file context hitches it to that
    /// file, a folder makes it project-wide); otherwise the note moves to
    /// the context's notes file. Returns the note's file and the note.
    pub fn rehitch(
        &self,
        from: &Path,
        id: &str,
        to: &Context,
    ) -> Result<(PathBuf, Note), StoreError> {
        let target = self.notes_path(to);
        let files = match to {
            Context::File { file, .. } => vec![file.clone()],
            _ => Vec::new(),
        };
        if normalize(from) == normalize(&target) {
            let note = self.modify(from, |file| {
                let note = file.get_mut(id).ok_or_else(|| not_found(from, id))?;
                note.files = files;
                Ok(note.clone())
            })?;
            return Ok((target, note));
        }
        let moved = self.move_note(from, id, &target)?;
        let id = moved.id.clone().unwrap_or_default();
        let note = self.modify(&target, |file| {
            let note = file.get_mut(&id).ok_or_else(|| not_found(&target, &id))?;
            note.files = files;
            Ok(note.clone())
        })?;
        Ok((target, note))
    }

    /// Load, change and write back a notes file, under the write lock.
    /// Writes nothing when the change leaves the text as it was.
    fn modify<T>(
        &self,
        path: &Path,
        change: impl FnOnce(&mut NotesFile) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let old = read_text(path)?;
        let mut file = old
            .as_deref()
            .map(NotesFile::parse)
            .unwrap_or_else(NotesFile::new);
        let out = change(&mut file)?;
        let text = file.render();
        if old.as_deref() != Some(text.as_str()) {
            if old.is_none() {
                self.create_parent(path)?;
            }
            write_atomic(path, &text)?;
        }
        self.remember(path);
        Ok(out)
    }

    /// Fallback folders are created on demand; a project folder must exist.
    fn create_parent(&self, path: &Path) -> Result<(), StoreError> {
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        if !parent.exists() && path.starts_with(&self.config.fallback_dir) {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::io(parent, e))?;
        }
        Ok(())
    }

    // --- known locations -------------------------------------------------------

    /// Record a notes file in `locations.json`. Failing to record it only
    /// costs search coverage, so errors are ignored.
    fn remember(&self, path: &Path) {
        if path.starts_with(&self.config.fallback_dir) && self.is_fallback(path) {
            return; // found by listing the fallback folders
        }
        let path = normalize(path);
        let mut guard = self.locations.lock().unwrap_or_else(|e| e.into_inner());
        let locations = guard.get_or_insert_with(|| Locations::load(self.config.locations_path()));
        let _ = locations.add(&path);
    }

    fn is_fallback(&self, path: &Path) -> bool {
        let parent = path.parent();
        parent == Some(&self.config.apps_dir()) || parent == Some(&self.config.sites_dir())
    }

    /// Every notes file Hitch knows of: the known locations that still
    /// exist, plus every fallback file. Search and the watcher cover these.
    pub fn known_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = {
            let mut guard = self.locations.lock().unwrap_or_else(|e| e.into_inner());
            let locations = guard.insert(Locations::load(self.config.locations_path()));
            locations
                .files()
                .filter(|f| f.is_file())
                .map(Path::to_path_buf)
                .collect()
        };
        for dir in [self.config.apps_dir(), self.config.sites_dir()] {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut found: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "md") && p.is_file())
                .collect();
            found.sort();
            files.extend(found);
        }
        files
    }

    /// Drop known locations whose notes file is gone.
    pub fn prune_locations(&self) -> Result<Vec<PathBuf>, StoreError> {
        let mut guard = self.locations.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .get_or_insert_with(|| Locations::load(self.config.locations_path()))
            .prune()
    }
}

fn not_found(path: &Path, id: &str) -> StoreError {
    StoreError::NoteNotFound {
        path: path.to_path_buf(),
        id: id.to_owned(),
    }
}

/// The file's text, or `None` if it doesn't exist.
fn read_text(path: &Path) -> Result<Option<String>, StoreError> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| StoreError::NotUtf8(path.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StoreError::io(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        store: Store,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let store = Store::new(StoreConfig::new(root.join(".hitch")));
        Fixture {
            _tmp: tmp,
            root,
            store,
        }
    }

    fn project(root: &Path) -> PathBuf {
        let p = root.join("boodle");
        fs::create_dir_all(p.join("src")).unwrap();
        fs::create_dir(p.join(".git")).unwrap();
        fs::write(p.join("src/parser.rs"), "").unwrap();
        p
    }

    #[test]
    fn capture_then_show_for_a_file() {
        let f = fixture();
        let p = project(&f.root);
        let ctx = f.store.context_for_path(&p.join("src/parser.rs"));
        assert_eq!(
            ctx,
            Context::File {
                project: p.clone(),
                file: "src/parser.rs".into()
            }
        );
        assert!(f.store.notes_for(&ctx).unwrap().is_empty());

        let path = f.store.notes_path(&ctx);
        assert_eq!(path, p.join("HITCH.md"));
        let wide = f
            .store
            .save_note(&path, Note::new("Dev setup", "cargo run"))
            .unwrap();
        let tagged = f
            .store
            .save_note(
                &path,
                Note {
                    files: vec!["src/parser.rs".into()],
                    ..Note::new("Parser bug", "empty front matter")
                },
            )
            .unwrap();
        assert!(wide.id.is_some() && tagged.id.is_some());

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Hitch notes\n\n## Dev setup\n<!-- hitch id="));
        assert!(text.contains("file=src/parser.rs"));

        let groups = f.store.notes_for(&ctx).unwrap();
        assert_eq!(groups.len(), 1);
        let titles: Vec<_> = groups[0].notes.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Parser bug", "Dev setup"],
            "the file's notes come first"
        );

        // The project folder context shows file order, and the file and
        // folder contexts share one group.
        let folder = Context::Folder { path: p.clone() };
        let all = f.store.notes_for_all(&[ctx.clone(), folder]).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].context, ctx);

        // The file is now a known location.
        assert_eq!(f.store.known_files(), vec![path]);
    }

    #[test]
    fn edit_and_delete_keep_other_sections_byte_for_byte() {
        let f = fixture();
        let p = project(&f.root);
        let path = p.join("HITCH.md");
        let original = "# Notes\n\nHand-written intro.\n\n## Keep me\nas   is  \n\n## Edit me\nold\n\n## Delete me\nbye\n";
        fs::write(&path, original).unwrap();

        let groups = f
            .store
            .notes_for(&Context::Folder { path: p.clone() })
            .unwrap();
        let mut edit = groups[0].notes[1].clone();
        let delete = groups[0].notes[2].id.clone().unwrap();
        edit.body = "new".into();
        let saved = f.store.save_note(&path, edit.clone()).unwrap();
        assert_eq!(saved.id, edit.id);
        assert!(f.store.delete_note(&path, &delete).unwrap());
        assert!(!f.store.delete_note(&path, &delete).unwrap());

        let id = edit.id.unwrap();
        assert_eq!(
            saved.created, None,
            "no date is invented for a hand-written note"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!(
                "# Notes\n\nHand-written intro.\n\n## Keep me\nas   is  \n\n## Edit me\n<!-- hitch id={id} -->\nnew\n"
            )
        );
    }

    #[test]
    fn saving_an_unchanged_note_does_not_touch_the_file() {
        let f = fixture();
        let p = project(&f.root);
        let path = p.join("HITCH.md");
        fs::write(&path, "## Hand\nwritten\n").unwrap();
        let note = f.store.load(&path).unwrap().notes().next().unwrap().clone();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        f.store.save_note(&path, note).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "## Hand\nwritten\n");
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn app_and_site_fallbacks() {
        let f = fixture();
        let app = Context::App {
            app_id: "org.gnome.Nautilus".into(),
        };
        let site = Context::Url {
            url: "https://admin.example.com/billing".into(),
        };
        let app_path = f.store.notes_path(&app);
        assert_eq!(app_path, f.root.join(".hitch/apps/org.gnome.Nautilus.md"));
        f.store
            .save_note(&app_path, Note::new("Shortcut", "Ctrl+L"))
            .unwrap();
        let site_path = f.store.notes_path(&site);
        assert_eq!(site_path, f.root.join(".hitch/sites/admin.example.com.md"));
        f.store
            .save_note(&site_path, Note::new("Login", "use SSO"))
            .unwrap();

        assert_eq!(f.store.notes_for(&app).unwrap()[0].notes[0].body, "Ctrl+L");
        assert_eq!(f.store.known_files(), vec![app_path, site_path]);
        // Fallback files are found by listing, not recorded.
        assert!(!f.store.config().locations_path().exists());
    }

    #[test]
    fn project_folder_must_exist() {
        let f = fixture();
        let path = f.root.join("gone/HITCH.md");
        let err = f.store.save_note(&path, Note::new("a", "b")).unwrap_err();
        assert!(matches!(err, StoreError::Io { .. }), "{err}");
    }

    #[test]
    fn rehitch_within_a_file_and_across_files() {
        let f = fixture();
        let p = project(&f.root);
        let path = p.join("HITCH.md");
        let note = f.store.save_note(&path, Note::new("Tip", "x")).unwrap();
        let id = note.id.clone().unwrap();

        let file_ctx = Context::File {
            project: p.clone(),
            file: "src/parser.rs".into(),
        };
        let (where_, n) = f.store.rehitch(&path, &id, &file_ctx).unwrap();
        assert_eq!(where_, path);
        assert_eq!(n.files, vec![PathBuf::from("src/parser.rs")]);

        let app = Context::App {
            app_id: "figma".into(),
        };
        let (where_, n) = f.store.rehitch(&path, &id, &app).unwrap();
        assert_eq!(where_, f.root.join(".hitch/apps/figma.md"));
        assert!(n.files.is_empty());
        assert_eq!(n.id.as_deref(), Some(id.as_str()));
        assert!(f.store.load(&path).unwrap().is_empty());
        assert!(matches!(
            f.store.rehitch(&path, &id, &app),
            Err(StoreError::NoteNotFound { .. })
        ));
    }

    #[test]
    fn moving_onto_a_clashing_id_gets_a_new_id() {
        let f = fixture();
        let p = project(&f.root);
        let a = p.join("HITCH.md");
        let b = f.store.notes_path(&Context::App { app_id: "x".into() });
        fs::write(&a, "## Todo\nfrom a\n").unwrap();
        f.store.create_parent(&b).unwrap();
        fs::write(&b, "## Todo\nfrom b\n").unwrap();
        let id = f
            .store
            .load(&a)
            .unwrap()
            .notes()
            .next()
            .unwrap()
            .id
            .clone()
            .unwrap();
        let moved = f.store.move_note(&a, &id, &b).unwrap();
        assert_ne!(moved.id.as_deref(), Some(id.as_str()));
        let bodies: Vec<_> = f
            .store
            .load(&b)
            .unwrap()
            .notes()
            .map(|n| n.body.clone())
            .collect();
        assert_eq!(bodies, ["from b", "from a"]);
    }

    #[test]
    fn settings_override_file_name_and_fallback_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".hitch");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            r#"{"notes_file_name":"NOTES.md","fallback_dir":"/srv/notes","panel":{"pinned":true}}"#,
        )
        .unwrap();
        let c = StoreConfig::from_settings(&dir);
        assert_eq!(c.file_name, "NOTES.md");
        assert_eq!(c.apps_dir(), PathBuf::from("/srv/notes/apps"));
        assert_eq!(c.locations_path(), dir.join("locations.json"));

        fs::write(dir.join("settings.json"), r#"{"notes_file_name":"../x"}"#).unwrap();
        assert_eq!(StoreConfig::from_settings(&dir).file_name, "HITCH.md");
    }

    #[test]
    fn non_utf8_files_are_left_alone() {
        let f = fixture();
        let p = project(&f.root);
        let path = p.join("HITCH.md");
        fs::write(&path, b"## A\n\xff\xfe\n").unwrap();
        assert!(matches!(f.store.load(&path), Err(StoreError::NotUtf8(_))));
        assert!(f.store.save_note(&path, Note::new("b", "c")).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"## A\n\xff\xfe\n");
    }
}
