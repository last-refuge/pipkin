// Ported from coding-agent/test/resource-loader.test.ts (context files, nested worktree
// dedup), coding-agent/test/skills.test.ts (with its fixtures written inline) and
// coding-agent/test/prompt-templates.test.ts, plus discovery-order, symlink and ignore-file
// cases.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Context files under `root` (ancestors above a temp dir are outside the test's control).
fn under(files: Vec<ContextFile>, root: &Path) -> Vec<(PathBuf, String)> {
    files
        .into_iter()
        .filter(|f| f.path.starts_with(root))
        .map(|f| (f.path, f.content))
        .collect()
}

fn contents(files: Vec<ContextFile>, root: &Path) -> Vec<String> {
    under(files, root).into_iter().map(|(_, c)| c).collect()
}

// ─── Context files ─────────────────────────────────────────────────────────

#[test]
fn context_files_are_global_first_then_root_most_to_cwd_one_per_dir() {
    let t = tmp();
    let root = t.path();
    let agent = root.join("agent");
    let cwd = root.join("project");
    let nested = cwd.join("service");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(&agent).unwrap();
    write(&agent.join("AGENTS.md"), "global instructions");
    write(&agent.join("AGENTS.override.md"), "global override");
    write(&cwd.join("AGENTS.md"), "project instructions");
    write(
        &cwd.join("CLAUDE.md"),
        "project claude (shadowed by AGENTS.md)",
    );
    write(&nested.join("AGENTS.md"), "service instructions");
    write(&nested.join("AGENTS.override.md"), "service override");
    write(&root.join("CLAUDE.MD"), "root upper-case claude");

    let files = load_context_files(&nested, &agent);
    assert_eq!(files[0].path, agent.join("AGENTS.override.md"));
    assert_eq!(
        under(files, root),
        vec![
            (
                agent.join("AGENTS.override.md"),
                "global override".to_string()
            ),
            (
                root.join(if root.join("CLAUDE.md").exists() {
                    "CLAUDE.md"
                } else {
                    "CLAUDE.MD"
                }),
                "root upper-case claude".to_string(),
            ),
            (cwd.join("AGENTS.md"), "project instructions".to_string()),
            (
                nested.join("AGENTS.override.md"),
                "service override".to_string()
            ),
        ]
    );
}

#[test]
fn context_file_candidates_that_are_directories_are_skipped_and_bom_is_stripped() {
    let t = tmp();
    let cwd = t.path().join("p");
    fs::create_dir_all(cwd.join("AGENTS.override.md")).unwrap();
    fs::create_dir_all(cwd.join("AGENTS.md")).unwrap();
    write(&cwd.join("CLAUDE.md"), "\u{FEFF}Fallback instructions");
    let files = load_context_files(&cwd, &t.path().join("no-agent"));
    assert_eq!(
        under(files, t.path()),
        vec![(cwd.join("CLAUDE.md"), "Fallback instructions".to_string())]
    );
}

#[test]
fn a_config_dir_that_is_also_an_ancestor_is_loaded_once_first() {
    let t = tmp();
    let agent = t.path().join("a");
    let cwd = agent.join("b");
    fs::create_dir_all(&cwd).unwrap();
    write(&agent.join("AGENTS.md"), "global");
    write(&cwd.join("AGENTS.md"), "leaf");
    assert_eq!(
        contents(load_context_files(&cwd, &agent), t.path()),
        ["global", "leaf"]
    );
}

#[test]
fn a_symlinked_cwd_walks_the_link_path_not_the_target() {
    let t = tmp();
    let real = t.path().join("real").join("proj");
    let links = t.path().join("links");
    fs::create_dir_all(&real).unwrap();
    fs::create_dir_all(&links).unwrap();
    write(&real.join("AGENTS.md"), "project");
    write(&t.path().join("real").join("AGENTS.md"), "real parent");
    write(&links.join("AGENTS.md"), "link parent");
    std::os::unix::fs::symlink(&real, links.join("proj")).unwrap();
    let files = load_context_files(&links.join("proj"), &t.path().join("none"));
    assert_eq!(
        under(files, t.path()),
        vec![
            (links.join("AGENTS.md"), "link parent".to_string()),
            (links.join("proj").join("AGENTS.md"), "project".to_string()),
        ]
    );
    // A relative or dotted cwd is resolved lexically.
    let dotted = links.join("proj").join("..").join("proj").join(".");
    assert_eq!(
        contents(
            load_context_files(&dotted, &t.path().join("none")),
            t.path()
        ),
        ["link parent", "project"]
    );
}

/// The main repo's `.git/worktrees/<name>/` with `HEAD` and `commondir`, and the worktree's
/// `.git` file pointing at it.
fn link_worktree(main: &Path, worktree: &Path, name: &str) {
    let git_dir = main.join(".git").join("worktrees").join(name);
    fs::create_dir_all(&git_dir).unwrap();
    write(&main.join(".git").join("HEAD"), "ref: refs/heads/main\n");
    write(&git_dir.join("HEAD"), "ref: refs/heads/feat\n");
    write(&git_dir.join("commondir"), "../..");
    write(
        &worktree.join(".git"),
        &format!("gitdir: {}\n", git_dir.display()),
    );
}

struct Nested {
    _t: tempfile::TempDir,
    root: PathBuf,
    outer: PathBuf,
    main: PathBuf,
    worktree: PathBuf,
    src: PathBuf,
}

fn nested_worktree() -> Nested {
    let t = tmp();
    let root = t.path().to_path_buf();
    let outer = root.join("outer");
    let main = outer.join("main");
    let worktree = main.join("worktrees").join("feat");
    let src = worktree.join("src");
    fs::create_dir_all(&src).unwrap();
    link_worktree(&main, &worktree, "feat");
    Nested {
        _t: t,
        root,
        outer,
        main,
        worktree,
        src,
    }
}

fn agent_dir(root: &Path) -> PathBuf {
    root.join("agent-dir")
}

#[test]
fn nested_worktree_skips_the_main_repo_duplicate() {
    let n = nested_worktree();
    write(&n.main.join("AGENTS.md"), "main repo instructions");
    write(&n.worktree.join("AGENTS.md"), "worktree instructions");
    let files = load_context_files(&n.src, &agent_dir(&n.root));
    assert_eq!(contents(files, &n.root), ["worktree instructions"]);
}

#[test]
fn nested_worktree_inherits_main_context_when_it_has_none() {
    let n = nested_worktree();
    write(&n.main.join("AGENTS.md"), "main repo instructions");
    let files = load_context_files(&n.src, &agent_dir(&n.root));
    assert_eq!(contents(files, &n.root), ["main repo instructions"]);
}

#[test]
fn nested_worktree_only_skips_the_same_file_name() {
    let n = nested_worktree();
    write(&n.main.join("CLAUDE.md"), "main repo instructions");
    write(&n.worktree.join("AGENTS.md"), "worktree instructions");
    let files = load_context_files(&n.src, &agent_dir(&n.root));
    assert_eq!(
        contents(files, &n.root),
        ["main repo instructions", "worktree instructions"]
    );
}

#[test]
fn nested_worktree_keeps_ancestors_above_the_main_repo() {
    let n = nested_worktree();
    write(&n.outer.join("AGENTS.md"), "outer instructions");
    write(&n.main.join("AGENTS.md"), "main repo instructions");
    write(&n.worktree.join("AGENTS.md"), "worktree instructions");
    let files = load_context_files(&n.src, &agent_dir(&n.root));
    assert_eq!(
        contents(files, &n.root),
        ["outer instructions", "worktree instructions"]
    );
}

#[test]
fn bare_layout_shadows_nothing() {
    let t = tmp();
    let proj = t.path().join("proj");
    let bare = proj.join(".bare");
    let worktree = proj.join("main");
    let wt_git = bare.join("worktrees").join("main");
    fs::create_dir_all(&wt_git).unwrap();
    fs::create_dir_all(&worktree).unwrap();
    write(&bare.join("HEAD"), "ref: refs/heads/main\n");
    write(&wt_git.join("HEAD"), "ref: refs/heads/main\n");
    write(&wt_git.join("commondir"), "../..");
    write(
        &worktree.join(".git"),
        &format!("gitdir: {}\n", wt_git.display()),
    );
    write(&proj.join("AGENTS.md"), "container instructions");
    write(&worktree.join("AGENTS.md"), "worktree instructions");
    let files = load_context_files(&worktree, &t.path().join("agent"));
    assert_eq!(
        contents(files, t.path()),
        ["container instructions", "worktree instructions"]
    );
}

#[test]
fn sibling_worktree_shadows_nothing() {
    let t = tmp();
    let outer = t.path().join("outer");
    let main = outer.join("main");
    let sib = outer.join("sib-feat");
    fs::create_dir_all(sib.join("src")).unwrap();
    fs::create_dir_all(&main).unwrap();
    write(&outer.join("AGENTS.md"), "outer instructions");
    write(&sib.join("AGENTS.md"), "sibling worktree instructions");
    link_worktree(&main, &sib, "sib");
    let files = load_context_files(&sib.join("src"), &t.path().join("agent"));
    assert_eq!(
        contents(files, t.path()),
        ["outer instructions", "sibling worktree instructions"]
    );
}

#[test]
fn submodule_shadows_nothing() {
    let t = tmp();
    let sup = t.path().join("super");
    let sub = sup.join("vendor").join("lib");
    fs::create_dir_all(sub.join("src")).unwrap();
    write(&sup.join("AGENTS.md"), "superproject instructions");
    write(&sub.join("AGENTS.md"), "submodule instructions");
    let sub_git = sup.join(".git").join("modules").join("vendor").join("lib");
    fs::create_dir_all(&sub_git).unwrap();
    write(&sub_git.join("HEAD"), "ref: refs/heads/main\n");
    write(
        &sub.join(".git"),
        &format!("gitdir: {}\n", sub_git.display()),
    );
    let files = load_context_files(&sub.join("src"), &t.path().join("agent"));
    assert_eq!(
        contents(files, t.path()),
        ["superproject instructions", "submodule instructions"]
    );
}

#[test]
fn ordinary_repo_root_does_not_stop_the_walk() {
    let t = tmp();
    let outer = t.path().join("outer");
    let repo = outer.join("repo");
    let leaf = repo.join("src");
    fs::create_dir_all(&leaf).unwrap();
    write(&repo.join(".git").join("HEAD"), "ref: refs/heads/main\n");
    write(&outer.join("AGENTS.md"), "outer instructions");
    write(&repo.join("AGENTS.md"), "repo instructions");
    write(&leaf.join("AGENTS.md"), "leaf instructions");
    let files = load_context_files(&leaf, &t.path().join("agent"));
    assert_eq!(
        contents(files, t.path()),
        [
            "outer instructions",
            "repo instructions",
            "leaf instructions"
        ]
    );
}

// ─── Skills ────────────────────────────────────────────────────────────────

/// coding-agent/test/fixtures/skills, verbatim.
fn write_skill_fixtures(dir: &Path) {
    let files: &[(&str, &str)] = &[
        (
            "consecutive-hyphens/SKILL.md",
            "---\nname: bad--name\ndescription: A skill with consecutive hyphens in the name.\n---\n\n# Consecutive Hyphens\n\nThis skill has consecutive hyphens in its name.\n",
        ),
        (
            "disable-model-invocation/SKILL.md",
            "---\nname: disable-model-invocation\ndescription: A skill that cannot be invoked by the model.\ndisable-model-invocation: true\n---\n\n# Manual Only Skill\n\nThis skill can only be invoked via /skill:disable-model-invocation.\n",
        ),
        (
            "invalid-name-chars/SKILL.md",
            "---\nname: Invalid_Name\ndescription: A skill with invalid characters in the name.\n---\n\n# Invalid Name\n\nThis skill has uppercase and underscore in the name.\n",
        ),
        (
            "invalid-yaml/SKILL.md",
            "---\nname: invalid-yaml\ndescription: [unclosed bracket\n---\n\n# Invalid YAML Skill\n\nThis skill has invalid YAML in the frontmatter.\n",
        ),
        (
            "long-name/SKILL.md",
            "---\nname: this-is-a-very-long-skill-name-that-exceeds-the-sixty-four-character-limit-set-by-the-standard\ndescription: A skill with a name that exceeds 64 characters.\n---\n\n# Long Name\n\nThis skill's name is too long.\n",
        ),
        (
            "missing-description/SKILL.md",
            "---\nname: missing-description\n---\n\n# Missing Description\n\nThis skill has no description field.\n",
        ),
        (
            "multiline-description/SKILL.md",
            "---\nname: multiline-description\ndescription: |\n  This is a multiline description.\n  It spans multiple lines.\n  And should be normalized.\n---\n\n# Multiline Description Skill\n\nThis skill tests that multiline YAML descriptions are normalized to single lines.\n",
        ),
        (
            "name-mismatch/SKILL.md",
            "---\nname: different-name\ndescription: A skill with a name that doesn't match the directory.\n---\n\n# Name Mismatch\n\nThis skill's name doesn't match its parent directory.\n",
        ),
        (
            "nested/child-skill/SKILL.md",
            "---\nname: child-skill\ndescription: A nested skill in a subdirectory.\n---\n\n# Child Skill\n\nThis skill is nested in a subdirectory.\n",
        ),
        (
            "no-frontmatter/SKILL.md",
            "# No Frontmatter\n\nThis skill has no YAML frontmatter at all.\n",
        ),
        (
            "root-skill-preferred/nested-child/SKILL.md",
            "---\ndescription: Nested skill should be ignored.\n---\n",
        ),
        (
            "root-skill-preferred/SKILL.md",
            "---\ndescription: Root skill should win.\n---\n",
        ),
        (
            "unknown-field/SKILL.md",
            "---\nname: unknown-field\ndescription: A skill with an unknown frontmatter field.\nauthor: someone\nversion: 1.0\n---\n\n# Unknown Field\n\nThis skill has non-standard frontmatter fields.\n",
        ),
        (
            "valid-skill/SKILL.md",
            "---\nname: valid-skill\ndescription: A valid skill for testing purposes.\n---\n\n# Valid Skill\n\nThis is a valid skill that follows the Agent Skills standard.\n",
        ),
    ];
    for (path, content) in files {
        write(&dir.join(path), content);
    }
}

/// `loadSkillsFromDir({dir})` through the public loader: one explicit path, empty defaults.
fn load_from(dir: &Path) -> (Vec<Skill>, Vec<String>) {
    let t = tmp();
    load_skills(
        &t.path().join("cwd"),
        &t.path().join("agent"),
        &[dir.to_string_lossy().into_owned()],
    )
}

struct Fixtures {
    _t: tempfile::TempDir,
    dir: PathBuf,
}

fn fixtures() -> Fixtures {
    let t = tmp();
    let dir = t.path().join("skills");
    write_skill_fixtures(&dir);
    Fixtures { _t: t, dir }
}

#[test]
fn skill_fixture_cases() {
    let f = fixtures();
    let one = |name: &str| load_from(&f.dir.join(name));

    let (skills, warnings) = one("valid-skill");
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "valid-skill");
    assert_eq!(skills[0].description, "A valid skill for testing purposes.");
    assert_eq!(skills[0].source, SkillSource::Path);
    assert!(!skills[0].disable_model_invocation);
    assert_eq!(skills[0].base_dir, f.dir.join("valid-skill"));
    assert!(warnings.is_empty(), "{warnings:?}");

    let (skills, warnings) = one("name-mismatch");
    assert_eq!(skills[0].name, "different-name");
    assert!(warnings.is_empty());

    let (skills, warnings) = one("invalid-name-chars");
    assert_eq!(skills.len(), 1);
    assert!(warnings.iter().any(|w| w.contains("invalid characters")));

    let (skills, warnings) = one("long-name");
    assert_eq!(skills.len(), 1);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("exceeds 64 characters (94)"))
    );

    let (skills, warnings) = one("missing-description");
    assert!(skills.is_empty());
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("description is required"))
    );

    let (skills, warnings) = one("unknown-field");
    assert_eq!(skills.len(), 1);
    assert!(warnings.is_empty());

    let (skills, warnings) = one("nested");
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "child-skill");
    assert!(warnings.is_empty());

    let (skills, warnings) = one("root-skill-preferred");
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "root-skill-preferred");
    assert_eq!(skills[0].description, "Root skill should win.");
    assert!(warnings.is_empty());

    let (skills, warnings) = one("no-frontmatter");
    assert!(skills.is_empty());
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("description is required"))
    );

    let (skills, warnings) = one("invalid-yaml");
    assert!(skills.is_empty());
    assert!(
        warnings.iter().any(|w| w.contains("at line")),
        "{warnings:?}"
    );

    let (skills, warnings) = one("multiline-description");
    assert_eq!(skills.len(), 1);
    assert!(skills[0].description.contains('\n'));
    assert!(
        skills[0]
            .description
            .contains("This is a multiline description.")
    );
    assert!(warnings.is_empty());

    let (skills, warnings) = one("consecutive-hyphens");
    assert_eq!(skills.len(), 1);
    assert!(warnings.iter().any(|w| w.contains("consecutive hyphens")));

    let (skills, _) = one("disable-model-invocation");
    assert!(skills[0].disable_model_invocation);
}

#[test]
fn whole_fixture_directory_loads_every_described_skill_in_name_order() {
    let f = fixtures();
    let (skills, _) = load_from(&f.dir);
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "bad--name",
            "disable-model-invocation",
            "Invalid_Name",
            "this-is-a-very-long-skill-name-that-exceeds-the-sixty-four-character-limit-set-by-the-standard",
            "multiline-description",
            "different-name",
            "child-skill",
            "root-skill-preferred",
            "unknown-field",
            "valid-skill",
        ]
    );
}

#[test]
fn missing_paths_and_non_markdown_files_warn() {
    let t = tmp();
    write(&t.path().join("notes.txt"), "x");
    let (skills, warnings) = load_skills(
        &t.path().join("cwd"),
        &t.path().join("agent"),
        &[
            "/non/existent/path".to_string(),
            t.path().join("notes.txt").to_string_lossy().into_owned(),
        ],
    );
    assert!(skills.is_empty());
    assert_eq!(
        warnings,
        [
            "/non/existent/path: skill path does not exist".to_string(),
            format!(
                "{}: skill path is not a markdown file",
                t.path().join("notes.txt").display()
            ),
        ]
    );
}

#[test]
fn discovery_order_is_config_then_project_then_settings_paths_and_first_name_wins() {
    let t = tmp();
    let agent = t.path().join("agent");
    let cwd = t.path().join("cwd");
    write(
        &agent.join("skills/calendar/SKILL.md"),
        "---\nname: calendar\ndescription: First calendar skill.\n---\n",
    );
    write(
        &cwd.join(".pi/skills/calendar/SKILL.md"),
        "---\nname: calendar\ndescription: Second calendar skill.\n---\n",
    );
    write(
        &cwd.join(".pi/skills/notes/SKILL.md"),
        "---\ndescription: Notes.\n---\n",
    );
    write(
        &cwd.join("extra/tool.md"),
        "---\nname: tool\ndescription: A loose file skill.\n---\n",
    );
    let (skills, warnings) = load_skills(
        &cwd,
        &agent,
        &["extra".to_string(), "extra/tool.md".to_string()],
    );
    let summary: Vec<(&str, SkillSource, &str)> = skills
        .iter()
        .map(|s| (s.name.as_str(), s.source, s.description.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            ("calendar", SkillSource::User, "First calendar skill."),
            ("notes", SkillSource::Project, "Notes."),
            ("tool", SkillSource::Path, "A loose file skill."),
        ]
    );
    // The same file through two settings paths is deduplicated silently; the name collision
    // is reported.
    assert_eq!(
        warnings,
        [format!(
            "{}: name \"calendar\" collision (kept {})",
            cwd.join(".pi/skills/calendar/SKILL.md").display(),
            agent.join("skills/calendar/SKILL.md").display()
        )]
    );
}

#[test]
fn loose_markdown_counts_only_at_the_scan_root_and_dot_dirs_and_node_modules_are_skipped() {
    let t = tmp();
    let dir = t.path().join("s");
    write(
        &dir.join("root.md"),
        "---\nname: root\ndescription: Root file.\n---\n",
    );
    write(
        &dir.join("readme.md"),
        "No frontmatter, not a skill and no warning.\n",
    );
    write(
        &dir.join("sub/loose.md"),
        "---\nname: loose\ndescription: Below the root.\n---\n",
    );
    write(
        &dir.join(".hidden/SKILL.md"),
        "---\nname: hidden\ndescription: Hidden.\n---\n",
    );
    write(
        &dir.join("node_modules/dep/SKILL.md"),
        "---\nname: dep\ndescription: Dep.\n---\n",
    );
    let (skills, warnings) = load_from(&dir);
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["root"]);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn ignore_files_are_honored_with_directory_prefixes() {
    let t = tmp();
    let dir = t.path().join("s");
    write(&dir.join(".gitignore"), "# comment\ndrafts/\n");
    write(
        &dir.join("drafts/x/SKILL.md"),
        "---\nname: x\ndescription: Draft.\n---\n",
    );
    write(&dir.join("team/.ignore"), "old\n");
    write(
        &dir.join("team/old/SKILL.md"),
        "---\nname: old\ndescription: Old.\n---\n",
    );
    write(
        &dir.join("team/new/SKILL.md"),
        "---\nname: new\ndescription: New.\n---\n",
    );
    // A prefixed rule only applies below its own directory.
    write(
        &dir.join("old/SKILL.md"),
        "---\nname: top-old\ndescription: Not ignored.\n---\n",
    );
    write(
        &dir.join("Keep/SKILL.md"),
        "---\nname: keep\ndescription: Kept.\n---\n",
    );
    write(&dir.join(".fdignore"), "KEEP-not-a-match\n");
    let (skills, _) = load_from(&dir);
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["keep", "top-old", "new"]);
}

#[test]
fn symlinked_skills_are_followed_and_deduplicated_by_real_path() {
    let t = tmp();
    let real = t.path().join("real/linked");
    write(
        &real.join("SKILL.md"),
        "---\nname: linked\ndescription: Via a link.\n---\n",
    );
    let dir = t.path().join("s");
    fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(&real, dir.join("a-link")).unwrap();
    std::os::unix::fs::symlink(&real, dir.join("b-link")).unwrap();
    std::os::unix::fs::symlink(t.path().join("missing"), dir.join("broken")).unwrap();
    // A cycle back to the scan root is not descended forever.
    std::os::unix::fs::symlink(&dir, dir.join("loop")).unwrap();
    let (skills, warnings) = load_from(&dir);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].file_path, dir.join("a-link/SKILL.md"));
    assert_eq!(skills[0].base_dir, dir.join("a-link"));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn format_skills_matches_the_reference_text() {
    let skill = |name: &str, description: &str, hidden: bool| Skill {
        name: name.into(),
        description: description.into(),
        file_path: PathBuf::from(format!("/path/{name}/SKILL.md")),
        base_dir: PathBuf::from(format!("/path/{name}")),
        source: SkillSource::Path,
        disable_model_invocation: hidden,
    };
    assert_eq!(format_skills(&[], "read"), "");
    assert_eq!(format_skills(&[skill("h", "d", true)], "read"), "");
    let text = format_skills(
        &[
            skill("one", "A skill with <special> & \"characters\" 'q'.", false),
            skill("hidden", "x", true),
            skill("two", "Second skill.", false),
        ],
        "bash",
    );
    assert_eq!(
        text,
        "\n\nThe following skills provide specialized instructions for specific tasks.\nUse bash to load a skill's file when the task matches its description.\nWhen a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.\n\n<available_skills>\n  <skill>\n    <name>one</name>\n    <description>A skill with &lt;special&gt; &amp; &quot;characters&quot; &apos;q&apos;.</description>\n    <location>/path/one/SKILL.md</location>\n  </skill>\n  <skill>\n    <name>two</name>\n    <description>Second skill.</description>\n    <location>/path/two/SKILL.md</location>\n  </skill>\n</available_skills>"
    );
    assert!(
        format_skills(&[skill("a", "b", false)], "read")
            .contains("Use the read tool to load a skill's file")
    );
}

#[test]
fn skill_commands_expand_to_the_reference_block() {
    let t = tmp();
    let file = t.path().join("deploy/SKILL.md");
    write(
        &file,
        "---\nname: deploy\ndescription: Deploy.\n---\n\n  Run ./ship.sh.\n\n",
    );
    let skills = vec![Skill {
        name: "deploy".into(),
        description: "Deploy.".into(),
        file_path: file.clone(),
        base_dir: t.path().join("deploy"),
        source: SkillSource::User,
        disable_model_invocation: true,
    }];
    let block = format!(
        "<skill name=\"deploy\" location=\"{}\">\nReferences are relative to {}.\n\nRun ./ship.sh.\n</skill>",
        file.display(),
        t.path().join("deploy").display()
    );
    assert_eq!(
        expand_skill_command("/skill:deploy", &skills),
        Ok(Some(block.clone()))
    );
    assert_eq!(
        expand_skill_command("/skill:deploy   to prod \n", &skills),
        Ok(Some(format!("{block}\n\nto prod")))
    );
    assert_eq!(
        expand_skill_command("/skill:deploy ", &skills),
        Ok(Some(block))
    );
    assert_eq!(expand_skill_command("/skill:other x", &skills), Ok(None));
    assert_eq!(expand_skill_command("deploy", &skills), Ok(None));
    // A newline does not end the name (only a space does), like the reference.
    assert_eq!(expand_skill_command("/skill:deploy\nx", &skills), Ok(None));
    fs::remove_file(&file).unwrap();
    assert!(expand_skill_command("/skill:deploy", &skills).is_err());
}

// ─── Prompt templates ──────────────────────────────────────────────────────

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn sub(template: &str, items: &[&str]) -> String {
    substitute_args(template, &args(items))
}

#[test]
fn substitute_args_reference_cases() {
    assert_eq!(sub("Test: $ARGUMENTS", &["a", "b", "c"]), "Test: a b c");
    assert_eq!(sub("Test: $@", &["a", "b", "c"]), "Test: a b c");
    assert_eq!(sub("$ARGUMENTS", &["$1", "$ARGUMENTS"]), "$1 $ARGUMENTS");
    assert_eq!(sub("$@", &["$100", "$1"]), "$100 $1");
    assert_eq!(sub("$ARGUMENTS", &["$100", "$1"]), "$100 $1");
    assert_eq!(
        sub("$1: $ARGUMENTS", &["prefix", "a", "b"]),
        "prefix: prefix a b"
    );
    assert_eq!(sub("$1: $@", &["prefix", "a", "b"]), "prefix: prefix a b");
    assert_eq!(sub("Test: $ARGUMENTS", &[]), "Test: ");
    assert_eq!(sub("Test: $@", &[]), "Test: ");
    assert_eq!(sub("Test: $1", &[]), "Test: ");
    assert_eq!(sub("$ARGUMENTS and $ARGUMENTS", &["a", "b"]), "a b and a b");
    assert_eq!(sub("$@ and $@", &["a", "b"]), "a b and a b");
    assert_eq!(sub("$@ and $ARGUMENTS", &["a", "b"]), "a b and a b");
    assert_eq!(
        sub("$1 $2: $ARGUMENTS", &["arg100", "@user"]),
        "arg100 @user: arg100 @user"
    );
    assert_eq!(sub("$1 $2 $3 $4 $5", &["a", "b"]), "a b   ");
    assert_eq!(
        sub("$ARGUMENTS", &["日本語", "🎉", "café"]),
        "日本語 🎉 café"
    );
    assert_eq!(
        sub("$1 $2", &["line1\nline2", "tab\tthere"]),
        "line1\nline2 tab\tthere"
    );
    assert_eq!(sub("$1$2", &["a", "b"]), "ab");
    assert_eq!(
        sub("$ARGUMENTS", &["first arg", "second arg"]),
        "first arg second arg"
    );
    assert_eq!(sub("Test: $ARGUMENTS", &["only"]), "Test: only");
    assert_eq!(sub("$0", &["a", "b"]), "");
    assert_eq!(sub("$1.5", &["a"]), "a.5");
    assert_eq!(sub("pre$ARGUMENTS", &["a", "b"]), "prea b");
    assert_eq!(sub("pre$@", &["a", "b"]), "prea b");
    assert_eq!(sub("$ARGUMENTS", &["a", "", "c"]), "a  c");
    assert_eq!(
        sub("$ARGUMENTS", &["  leading  ", "trailing  "]),
        "  leading   trailing  "
    );
    assert_eq!(
        sub("Prefix $ARGUMENTS suffix", &["ARGUMENTS"]),
        "Prefix ARGUMENTS suffix"
    );
    assert_eq!(sub("$A $$ $ $ARGS", &["a"]), "$A $$ $ $ARGS");
    assert_eq!(
        sub("$arguments $Arguments $ARGUMENTS", &["a", "b"]),
        "$arguments $Arguments a b"
    );
    let many: Vec<String> = (0..100).map(|i| format!("arg{i}")).collect();
    assert_eq!(substitute_args("$ARGUMENTS", &many), many.join(" "));
    let fifteen: Vec<String> = (0..15).map(|i| format!("val{i}")).collect();
    assert_eq!(substitute_args("$10 $12 $15", &fifteen), "val9 val11 val14");
    assert_eq!(sub("Price: \\$100", &[]), "Price: \\");
    assert_eq!(
        sub("$1: $@ ($ARGUMENTS)", &["first", "second", "third"]),
        "first: first second third (first second third)"
    );
    assert_eq!(sub("Just plain text", &["a", "b"]), "Just plain text");
    assert_eq!(sub("$1 $2 $@", &["a", "b", "c"]), "a b a b c");
    // Unicode digits are not placeholders (the reference's `\d` is ASCII).
    assert_eq!(sub("$١", &["a"]), "$١");
    assert_eq!(sub("$99999999999999999999999", &["a"]), "");
}

#[test]
fn substitute_args_positional_defaults() {
    assert_eq!(
        sub("List exactly ${1:-7} next steps", &[]),
        "List exactly 7 next steps"
    );
    let both = "${@:-default}\n${ARGUMENTS:-default}";
    assert_eq!(sub(both, &[]), "default\ndefault");
    assert_eq!(
        sub(both, &["This", "would", "be", "the", "arguments"]),
        "This would be the arguments\nThis would be the arguments"
    );
    assert_eq!(
        sub("List exactly ${1:-7} next steps", &["3"]),
        "List exactly 3 next steps"
    );
    assert_eq!(sub("Mode: ${1:-brief}", &[""]), "Mode: brief");
    assert_eq!(sub("${1:-7} ${2:-brief}", &[]), "7 brief");
    assert_eq!(sub("${1:-7} ${2:-brief}", &["3"]), "3 brief");
    assert_eq!(sub("${1:-7} ${2:-brief}", &["3", "verbose"]), "3 verbose");
    assert_eq!(sub("${1:-7}", &["$ARGUMENTS"]), "$ARGUMENTS");
    assert_eq!(sub("${1:-7}", &["$1"]), "$1");
    assert_eq!(sub("${1:-$ARGUMENTS}", &["a", "b"]), "a");
    assert_eq!(sub("${3:-$ARGUMENTS}", &["a", "b"]), "$ARGUMENTS");
    assert_eq!(sub("${1:-seven steps}", &[]), "seven steps");
    assert_eq!(sub("${3:-fallback}", &["a", "b"]), "fallback");
    assert_eq!(sub("$1 ${2:-x} $ARGUMENTS", &["a"]), "a x a");
}

#[test]
fn substitute_args_slices() {
    assert_eq!(sub("${@:2}", &["a", "b", "c", "d"]), "b c d");
    assert_eq!(sub("${@:1}", &["a", "b", "c"]), "a b c");
    assert_eq!(sub("${@:3}", &["a", "b", "c", "d"]), "c d");
    assert_eq!(sub("${@:2:2}", &["a", "b", "c", "d"]), "b c");
    assert_eq!(sub("${@:1:1}", &["a", "b", "c"]), "a");
    assert_eq!(sub("${@:3:1}", &["a", "b", "c", "d"]), "c");
    assert_eq!(sub("${@:2:3}", &["a", "b", "c", "d", "e"]), "b c d");
    assert_eq!(sub("${@:99}", &["a", "b"]), "");
    assert_eq!(sub("${@:5}", &["a", "b"]), "");
    assert_eq!(sub("${@:10:5}", &["a", "b"]), "");
    assert_eq!(sub("${@:2:0}", &["a", "b", "c"]), "");
    assert_eq!(sub("${@:1:0}", &["a", "b"]), "");
    assert_eq!(sub("${@:2:99}", &["a", "b", "c"]), "b c");
    assert_eq!(sub("${@:1:10}", &["a", "b"]), "a b");
    assert_eq!(sub("${@:2} vs $@", &["a", "b", "c"]), "b c vs a b c");
    assert_eq!(
        sub("First: ${@:1:1}, All: $@", &["x", "y", "z"]),
        "First: x, All: x y z"
    );
    assert_eq!(sub("${@:1}", &["${@:2}", "test"]), "${@:2} test");
    assert_eq!(sub("${@:2}", &["a", "${@:3}", "c"]), "${@:3} c");
    assert_eq!(
        sub("$1: ${@:2}", &["cmd", "arg1", "arg2"]),
        "cmd: arg1 arg2"
    );
    assert_eq!(sub("$1 $2 ${@:3}", &["a", "b", "c", "d"]), "a b c d");
    assert_eq!(sub("${@:0}", &["a", "b", "c"]), "a b c");
    assert_eq!(sub("${@:2}", &[]), "");
    assert_eq!(sub("${@:1}", &[]), "");
    assert_eq!(sub("${@:1}", &["only"]), "only");
    assert_eq!(sub("${@:2}", &["only"]), "");
    assert_eq!(
        sub("Process ${@:2} with $1", &["tool", "file1", "file2"]),
        "Process file1 file2 with tool"
    );
    assert_eq!(sub("${@:1:1} and ${@:2}", &["a", "b", "c"]), "a and b c");
    assert_eq!(
        sub("${@:1:2} vs ${@:3:2}", &["a", "b", "c", "d", "e"]),
        "a b vs c d"
    );
    assert_eq!(
        sub("${@:2}", &["cmd", "first arg", "second arg"]),
        "first arg second arg"
    );
    assert_eq!(
        sub("${@:2}", &["cmd", "$100", "@user", "#tag"]),
        "$100 @user #tag"
    );
    assert_eq!(sub("${@:1}", &["日本語", "🎉", "café"]), "日本語 🎉 café");
    assert_eq!(
        sub(
            "Run $1 on ${@:2:2}, then process $@",
            &["eslint", "file1.ts", "file2.ts", "file3.ts"]
        ),
        "Run eslint on file1.ts file2.ts, then process eslint file1.ts file2.ts file3.ts"
    );
    assert_eq!(
        sub("prefix${@:2}suffix", &["a", "b", "c"]),
        "prefixb csuffix"
    );
    let ten: Vec<String> = (1..=10).map(|i| format!("arg{i}")).collect();
    assert_eq!(
        substitute_args("${@:5:100}", &ten),
        "arg5 arg6 arg7 arg8 arg9 arg10"
    );
}

#[test]
fn parse_command_args_reference_cases() {
    assert_eq!(parse_command_args("a b c"), args(&["a", "b", "c"]));
    assert_eq!(
        parse_command_args("\"first arg\" second"),
        args(&["first arg", "second"])
    );
    assert_eq!(
        parse_command_args("'first arg' second"),
        args(&["first arg", "second"])
    );
    assert_eq!(
        parse_command_args("\"double\" 'single' \"double again\""),
        args(&["double", "single", "double again"])
    );
    assert!(parse_command_args("").is_empty());
    assert_eq!(parse_command_args("a  b   c"), args(&["a", "b", "c"]));
    assert_eq!(parse_command_args("a\tb\tc"), args(&["a", "b", "c"]));
    assert_eq!(parse_command_args("\"\" \" \""), args(&[" "]));
    assert_eq!(
        parse_command_args("$100 @user #tag"),
        args(&["$100", "@user", "#tag"])
    );
    assert_eq!(
        parse_command_args("日本語 🎉 café"),
        args(&["日本語", "🎉", "café"])
    );
    assert_eq!(
        parse_command_args("\"line1\nline2\" second"),
        args(&["line1\nline2", "second"])
    );
    assert_eq!(
        parse_command_args("label-2\n\nHere is some description #2."),
        args(&["label-2", "Here", "is", "some", "description", "#2."])
    );
    assert_eq!(parse_command_args("a\n\n\tb  c"), args(&["a", "b", "c"]));
    assert_eq!(
        parse_command_args("\"quoted \\\"text\\\"\""),
        args(&["quoted \\text\\"])
    );
    assert_eq!(parse_command_args("a b c   "), args(&["a", "b", "c"]));
    assert_eq!(parse_command_args("   a b c"), args(&["a", "b", "c"]));
    let parsed = parse_command_args("Button \"onClick handler\" \"disabled support\"");
    assert_eq!(
        substitute_args(
            "Create a React component named $1 with features: $ARGUMENTS",
            &parsed
        ),
        "Create a React component named Button with features: Button onClick handler disabled support"
    );
}

fn template(name: &str, content: &str) -> PromptTemplate {
    PromptTemplate {
        name: name.into(),
        description: "test".into(),
        argument_hint: None,
        content: content.into(),
        file_path: PathBuf::from(format!("/tmp/{name}.md")),
    }
}

#[test]
fn expand_template_cases() {
    let templates = vec![
        template("arg-test", "- arg1: $1\n- rest: ${@:2}"),
        template("one", "arg1: $1"),
    ];
    assert_eq!(
        expand_template(
            "/arg-test label-2\n\nHere is some description #2.",
            &templates
        )
        .as_deref(),
        Some("- arg1: label-2\n- rest: Here is some description #2.")
    );
    assert_eq!(
        expand_template("/one\nlabel-2", &templates).as_deref(),
        Some("arg1: label-2")
    );
    assert_eq!(
        expand_template("/one", &templates).as_deref(),
        Some("arg1: ")
    );
    assert_eq!(
        expand_template("/one   ", &templates).as_deref(),
        Some("arg1: ")
    );
    assert_eq!(expand_template("/unknown x", &templates), None);
    assert_eq!(expand_template("one x", &templates), None);
    assert_eq!(expand_template("/ one", &templates), None);
    assert_eq!(expand_template("/", &templates), None);
}

#[test]
fn templates_load_with_frontmatter_hints_descriptions_and_collisions() {
    let t = tmp();
    let agent = t.path().join("agent");
    let cwd = t.path().join("cwd");
    write(
        &agent.join("prompts/pr.md"),
        "---\ndescription: Review PRs from URLs with structured issue and code analysis\nargument-hint: \"<PR-URL>\"\n---\nYou are given one or more GitHub PR URLs: $@",
    );
    write(
        &agent.join("prompts/empty-hint.md"),
        "---\ndescription: A command with empty hint\nargument-hint: \"\"\n---\nDo something",
    );
    write(
        &agent.join("prompts/long.md"),
        &format!("\n\n   \n{}\nsecond", "d".repeat(61)),
    );
    write(&agent.join("prompts/short.md"), "First line\nsecond");
    write(&agent.join("prompts/notes.txt"), "not a template");
    fs::create_dir_all(agent.join("prompts/dir.md")).unwrap();
    write(
        &cwd.join(".pi/prompts/pr.md"),
        "---\ndescription: project pr\n---\nproject",
    );
    write(
        &cwd.join(".pi/prompts/invalid.md"),
        "---\ndescription: Broken: unquoted colon\n---\nDo something.\n",
    );
    write(&cwd.join(".pi/prompts/valid.md"), "Valid prompt content.");
    write(
        &cwd.join(".pi/prompts/sub/nested.md"),
        "Not scanned: non-recursive.",
    );

    let (templates, warnings) = load_templates(&cwd, &agent);
    let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["empty-hint", "long", "pr", "short", "valid"]);
    let pr = &templates[2];
    assert_eq!(pr.argument_hint.as_deref(), Some("<PR-URL>"));
    assert_eq!(
        pr.description,
        "Review PRs from URLs with structured issue and code analysis"
    );
    assert_eq!(pr.content, "You are given one or more GitHub PR URLs: $@");
    assert_eq!(templates[0].argument_hint, None);
    assert_eq!(templates[1].description, format!("{}...", "d".repeat(60)));
    assert_eq!(templates[3].description, "First line");
    assert_eq!(templates[4].description, "Valid prompt content.");

    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].starts_with(&format!(
        "{}: ",
        cwd.join(".pi/prompts/invalid.md").display()
    )));
    assert!(warnings[0].contains("line 1, column"), "{}", warnings[0]);
    assert_eq!(
        warnings[1],
        format!(
            "{}: name \"/pr\" collision (kept {})",
            cwd.join(".pi/prompts/pr.md").display(),
            agent.join("prompts/pr.md").display()
        )
    );
}
