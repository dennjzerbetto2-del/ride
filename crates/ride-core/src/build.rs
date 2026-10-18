//! Esecuzione dei comandi di build di un progetto Java: **Maven** e **Gradle**.
//!
//! L'editor non reimplementa nulla: si limita a riconoscere il sistema di build
//! dalla cartella radice (`pom.xml`, `build.gradle`, …) e a invocare lo
//! strumento standard. Se il progetto ha il *wrapper* (`mvnw` / `gradlew`) si
//! usa quello — è il modo giusto, perché fissa la versione dello strumento — e
//! altrimenti il comando di sistema (`mvn` / `gradle`).
//!
//! Qui c'è solo la parte "pura": riconoscere il tool e comporre la riga di
//! comando. L'avvio del processo (e la lettura dell'output) sta nella GUI, su un
//! thread separato, come per i formatter.

use std::path::{Path, PathBuf};

/// I sistemi di build riconosciuti.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuildTool {
    Maven,
    Gradle,
}

impl BuildTool {
    /// Nome da mostrare nei menu.
    pub const fn name(self) -> &'static str {
        match self {
            BuildTool::Maven => "Maven",
            BuildTool::Gradle => "Gradle",
        }
    }

    /// Il comando di sistema, cercato nel `PATH`.
    pub const fn program(self) -> &'static str {
        match self {
            BuildTool::Maven => "mvn",
            BuildTool::Gradle => "gradle",
        }
    }

    /// Lo script *wrapper* che può stare nella radice del progetto.
    ///
    /// Il secondo nome è quello usato su Windows.
    const fn wrappers(self) -> &'static [&'static str] {
        match self {
            BuildTool::Maven => &["mvnw", "mvnw.cmd"],
            BuildTool::Gradle => &["gradlew", "gradlew.bat"],
        }
    }

    /// Argomenti sempre presenti, per un output pulito da terminale (senza
    /// colori ANSI né barre di avanzamento che riscrivono la stessa riga).
    const fn prefix(self) -> &'static [&'static str] {
        match self {
            // `-B` = batch mode.
            BuildTool::Maven => &["-B"],
            BuildTool::Gradle => &["--console=plain"],
        }
    }
}

/// Un comando predefinito del menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildCommand {
    /// Voce di menu ("Compila", "Test", …).
    pub label: &'static str,
    /// Argomenti passati allo strumento (goal Maven / task Gradle).
    pub args: &'static [&'static str],
}

/// Come invocare concretamente il sistema di build di un progetto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    pub tool: BuildTool,
    /// Eseguibile: il wrapper nella radice oppure il nome nel `PATH`.
    pub program: PathBuf,
    /// `true` se `program` è il wrapper del progetto (percorso, non `PATH`).
    pub wrapper: bool,
}

impl Toolchain {
    /// `true` se lo strumento si può davvero avviare.
    pub fn available(&self) -> bool {
        if self.wrapper {
            self.program.is_file()
        } else {
            crate::format::is_available(&self.program.to_string_lossy())
        }
    }

    /// Nome breve per i messaggi ("mvnw", "gradle", …).
    pub fn label(&self) -> String {
        self.program
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.program.to_string_lossy().into_owned())
    }

    /// La riga di comando completa per un comando del menu.
    pub fn command_line(&self, command: &BuildCommand) -> Vec<String> {
        self.arguments(command.args)
    }

    /// Come [`Self::command_line`], per argomenti qualsiasi: i goal dei plugin e
    /// le loro opzioni (`dependency:tree -Dverbose`).
    pub fn arguments(&self, args: &[&str]) -> Vec<String> {
        self.tool
            .prefix()
            .iter()
            .chain(args.iter())
            .map(|arg| (*arg).to_string())
            .collect()
    }
}

/// I comandi proposti nel menu per un certo strumento.
pub fn commands(tool: BuildTool) -> &'static [BuildCommand] {
    match tool {
        BuildTool::Maven => &[
            BuildCommand { label: "Compila", args: &["compile"] },
            BuildCommand { label: "Test", args: &["test"] },
            BuildCommand { label: "Pacchetto", args: &["package"] },
            BuildCommand { label: "Verifica", args: &["verify"] },
            BuildCommand { label: "Installa", args: &["install"] },
            BuildCommand { label: "Pulisci", args: &["clean"] },
            BuildCommand { label: "Pulisci e compila", args: &["clean", "compile"] },
        ],
        BuildTool::Gradle => &[
            BuildCommand { label: "Compila", args: &["classes"] },
            BuildCommand { label: "Test", args: &["test"] },
            BuildCommand { label: "Costruisci", args: &["build"] },
            BuildCommand { label: "Esegui", args: &["run"] },
            BuildCommand { label: "Pulisci", args: &["clean"] },
            BuildCommand { label: "Pulisci e compila", args: &["clean", "classes"] },
        ],
    }
}

/// Un plugin Maven che l'editor conosce, con i goal che ha senso proporre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MavenPlugin {
    /// `artifactId` come compare nel `pom.xml`.
    pub artifact_id: &'static str,
    /// Prefisso dei goal: si lanciano come `mvn <prefisso>:<goal>`.
    pub prefix: &'static str,
    /// I goal proposti nel menu, nell'ordine in cui compaiono.
    pub goals: &'static [&'static str],
    /// `true` per i plugin che si possono usare **anche senza dichiararli**:
    /// Maven li risolve e li scarica da sé, quindi ha senso proporli sempre.
    /// Gli altri si propongono solo se il progetto li dichiara, altrimenti la
    /// voce di menu fallirebbe.
    pub always: bool,
}

impl MavenPlugin {
    /// L'argomento da passare a Maven per un goal (`dependency:tree`).
    pub fn goal(&self, goal: &str) -> String {
        format!("{}:{goal}", self.prefix)
    }
}

/// I plugin Maven proposti nel menu *Esegui*, in ordine di utilità.
///
/// È un elenco scelto a mano: i plugin che capita di lanciare da riga di comando
/// mentre si lavora, con i goal che ha senso invocare da soli. Non serve che sia
/// completo — [`project_plugins`] mostra solo quelli del progetto — e per tutto
/// il resto c'è la voce "Comando…", che accetta qualunque goal con le sue
/// opzioni.
pub const MAVEN_PLUGINS: &[MavenPlugin] = &[
    MavenPlugin {
        artifact_id: "spring-boot-maven-plugin",
        prefix: "spring-boot",
        goals: &["run", "start", "stop", "repackage", "build-image"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "exec-maven-plugin",
        prefix: "exec",
        goals: &["java", "exec"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-compiler-plugin",
        prefix: "compiler",
        goals: &["compile", "testCompile"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-surefire-plugin",
        prefix: "surefire",
        goals: &["test", "report-only"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-failsafe-plugin",
        prefix: "failsafe",
        goals: &["integration-test", "verify"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-resources-plugin",
        prefix: "resources",
        goals: &["resources", "testResources"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-jar-plugin",
        prefix: "jar",
        goals: &["jar", "test-jar"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-war-plugin",
        prefix: "war",
        goals: &["war", "exploded"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-shade-plugin",
        prefix: "shade",
        goals: &["shade"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-assembly-plugin",
        prefix: "assembly",
        goals: &["single", "assembly"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-dependency-plugin",
        prefix: "dependency",
        goals: &["tree", "analyze", "list", "build-classpath", "copy-dependencies", "go-offline"],
        always: true,
    },
    MavenPlugin {
        artifact_id: "maven-help-plugin",
        prefix: "help",
        goals: &["effective-pom", "describe", "evaluate", "active-profiles", "all-profiles"],
        always: true,
    },
    MavenPlugin {
        artifact_id: "maven-versions-plugin",
        prefix: "versions",
        goals: &[
            "display-dependency-updates",
            "display-plugin-updates",
            "display-property-updates",
            "use-latest-releases",
            "use-latest-versions",
        ],
        always: true,
    },
    MavenPlugin {
        artifact_id: "jacoco-maven-plugin",
        prefix: "jacoco",
        goals: &["prepare-agent", "report", "check", "report-aggregate"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-checkstyle-plugin",
        prefix: "checkstyle",
        goals: &["check", "checkstyle"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "spotless-maven-plugin",
        prefix: "spotless",
        goals: &["apply", "check"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-pmd-plugin",
        prefix: "pmd",
        goals: &["check", "pmd", "cpd-check"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "spotbugs-maven-plugin",
        prefix: "spotbugs",
        goals: &["check", "spotbugs", "gui"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-enforcer-plugin",
        prefix: "enforcer",
        goals: &["enforce", "display-info"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-javadoc-plugin",
        prefix: "javadoc",
        goals: &["javadoc", "jar", "aggregate"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-source-plugin",
        prefix: "source",
        goals: &["jar", "jar-no-fork", "aggregate"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-deploy-plugin",
        prefix: "deploy",
        goals: &["deploy"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-antrun-plugin",
        prefix: "antrun",
        goals: &["run"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-release-plugin",
        prefix: "release",
        goals: &["prepare", "perform", "rollback", "clean"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "flyway-maven-plugin",
        prefix: "flyway",
        goals: &["info", "migrate", "validate", "clean", "baseline", "repair"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "liquibase-maven-plugin",
        prefix: "liquibase",
        goals: &["update", "status", "validate", "updateSQL"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "jib-maven-plugin",
        prefix: "jib",
        goals: &["build", "dockerBuild", "buildTar"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "protobuf-maven-plugin",
        prefix: "protobuf",
        goals: &["compile", "test-compile", "compile-cpp"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "docker-maven-plugin",
        prefix: "docker",
        goals: &["build", "push", "start", "stop", "log"],
        always: false,
    },
    MavenPlugin {
        artifact_id: "maven-site-plugin",
        prefix: "site",
        goals: &["site", "run", "stage"],
        always: false,
    },
];

/// I plugin che il `pom.xml` dichiara, nell'ordine del catalogo.
///
/// Non è un parser XML: si riconoscono gli `artifactId` **noti** ovunque
/// compaiano. Un plugin elencato solo in `<pluginManagement>` o dentro un
/// profilo conta come dichiarato: va bene, il menu propone i suoi goal e
/// nient'altro.
pub fn pom_plugins(pom: &str) -> Vec<&'static MavenPlugin> {
    MAVEN_PLUGINS
        .iter()
        .filter(|plugin| declares(pom, plugin.artifact_id))
        .collect()
}

/// `true` se il testo dichiara esattamente quell'`artifactId`.
///
/// Il confronto è sul contenuto dell'elemento, non sul nome del file: l'importo
/// `maven-help-plugin` non deve far credere che ci sia
/// `maven-help-plugin-qualcosa`.
fn declares(pom: &str, artifact_id: &str) -> bool {
    const TAG: &str = "<artifactId>";
    pom.match_indices(TAG).any(|(at, tag)| {
        let after = pom[at + tag.len()..].trim_start();
        after
            .strip_prefix(artifact_id)
            .is_some_and(|rest| rest.trim_start().starts_with("</artifactId>"))
    })
}

/// I plugin da mostrare nel menu per un progetto Maven: prima quelli che il
/// `pom.xml` dichiara davvero, poi i pochi utili in ogni caso (albero delle
/// dipendenze, POM effettivo, aggiornamenti disponibili).
///
/// Se il `pom.xml` non è leggibile restano i plugin "sempre utili": meglio un
/// menu più corto che nessun menu.
pub fn project_plugins(root: &Path) -> Vec<&'static MavenPlugin> {
    let pom = std::fs::read_to_string(root.join("pom.xml")).unwrap_or_default();
    let mut chosen = pom_plugins(&pom);
    for plugin in MAVEN_PLUGINS.iter().filter(|plugin| plugin.always) {
        if !chosen.contains(&plugin) {
            chosen.push(plugin);
        }
    }
    chosen
}

/// `true` se la cartella contiene un progetto Maven.
pub fn is_maven(root: &Path) -> bool {
    root.join("pom.xml").is_file()
}

/// `true` se la cartella contiene un progetto Gradle.
pub fn is_gradle(root: &Path) -> bool {
    ["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"]
        .iter()
        .any(|name| root.join(name).is_file())
}

/// Come invocare il sistema di build indicato in questa cartella.
///
/// Non verifica che lo strumento esista: per quello c'è
/// [`Toolchain::available`]. Serve a poter mostrare le voci di menu anche
/// quando il comando non è installato, spiegando cosa manca.
pub fn toolchain_for(root: &Path, tool: BuildTool) -> Toolchain {
    match wrapper_path(root, tool) {
        Some(program) => Toolchain { tool, program, wrapper: true },
        None => Toolchain {
            tool,
            program: PathBuf::from(tool.program()),
            wrapper: false,
        },
    }
}

/// Il wrapper del progetto, se c'è.
fn wrapper_path(root: &Path, tool: BuildTool) -> Option<PathBuf> {
    tool.wrappers()
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
}

/// I sistemi di build presenti in una cartella (Maven e/o Gradle).
pub fn detect(root: &Path) -> Vec<Toolchain> {
    let mut tools = Vec::new();
    if is_maven(root) {
        tools.push(toolchain_for(root, BuildTool::Maven));
    }
    if is_gradle(root) {
        tools.push(toolchain_for(root, BuildTool::Gradle));
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(nome: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ride-build-{}-{nome}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn riconosce_maven_e_gradle() {
        let dir = temp_dir("detect");
        assert!(detect(&dir).is_empty());

        fs::write(dir.join("pom.xml"), "<project/>").unwrap();
        let tools = detect(&dir);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool, BuildTool::Maven);

        fs::write(dir.join("build.gradle.kts"), "").unwrap();
        let tools = detect(&dir);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[1].tool, BuildTool::Gradle);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preferisce_il_wrapper_del_progetto() {
        let dir = temp_dir("wrapper");
        fs::write(dir.join("pom.xml"), "<project/>").unwrap();

        // Senza wrapper si usa il comando di sistema.
        let plain = toolchain_for(&dir, BuildTool::Maven);
        assert!(!plain.wrapper);
        assert_eq!(plain.program, PathBuf::from("mvn"));
        assert_eq!(plain.label(), "mvn");

        // Con `mvnw` si usa quello.
        fs::write(dir.join("mvnw"), "#!/bin/sh\n").unwrap();
        let wrapped = toolchain_for(&dir, BuildTool::Maven);
        assert!(wrapped.wrapper);
        assert_eq!(wrapped.program, dir.join("mvnw"));
        assert!(wrapped.available());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn i_comandi_hanno_gli_argomenti_giusti() {
        let maven = &commands(BuildTool::Maven)[0];
        assert_eq!(maven.label, "Compila");
        assert_eq!(maven.args, &["compile"]);

        let gradle = commands(BuildTool::Gradle)
            .iter()
            .find(|c| c.label == "Costruisci")
            .unwrap();
        assert_eq!(gradle.args, &["build"]);

        // Il prefisso dipende dallo strumento e precede gli argomenti.
        let toolchain = Toolchain {
            tool: BuildTool::Maven,
            program: PathBuf::from("mvn"),
            wrapper: false,
        };
        let line = toolchain.command_line(&BuildCommand { label: "Test", args: &["test"] });
        assert_eq!(line, vec!["-B", "test"]);
    }

    #[test]
    fn il_pom_dice_quali_plugin_sono_in_uso() {
        let pom = r#"
<project>
  <build>
    <plugins>
      <plugin>
        <groupId>org.springframework.boot</groupId>
        <artifactId>spring-boot-maven-plugin</artifactId>
      </plugin>
      <plugin>
        <artifactId>
          maven-shade-plugin
        </artifactId>
      </plugin>
    </plugins>
  </build>
</project>
"#;
        let prefissi: Vec<&str> = pom_plugins(pom).iter().map(|p| p.prefix).collect();
        // Nell'ordine del catalogo, e con l'`artifactId` su più righe.
        assert_eq!(prefissi, vec!["spring-boot", "shade"]);
    }

    #[test]
    fn un_artifact_id_simile_non_conta() {
        // Un artefatto che *inizia* come un plugin noto non è quel plugin.
        assert!(pom_plugins("<artifactId>maven-help-plugin-extra</artifactId>").is_empty());
        // E nemmeno un plugin che non è nel catalogo.
        assert!(pom_plugins("<artifactId>qualcosa-maven-plugin</artifactId>").is_empty());
    }

    #[test]
    fn i_plugin_sempre_utili_ci_sono_anche_senza_pom() {
        let dir = temp_dir("plugin-senza-pom");
        let prefissi: Vec<&str> = project_plugins(&dir).iter().map(|p| p.prefix).collect();
        assert_eq!(prefissi, vec!["dependency", "help", "versions"]);

        // Con un `pom.xml` i plugin dichiarati vengono prima.
        fs::write(
            dir.join("pom.xml"),
            "<project><build><plugins><plugin>\
             <artifactId>spring-boot-maven-plugin</artifactId>\
             </plugin></plugins></build></project>",
        )
        .unwrap();
        let plugins = project_plugins(&dir);
        assert_eq!(plugins.len(), 4);
        assert_eq!(plugins[0].prefix, "spring-boot");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn un_goal_si_compone_col_prefisso() {
        let plugin = MAVEN_PLUGINS
            .iter()
            .find(|p| p.prefix == "dependency")
            .unwrap();
        assert_eq!(plugin.goal("tree"), "dependency:tree");
    }

    #[test]
    fn gli_argomenti_liberi_hanno_il_prefisso() {
        let toolchain = Toolchain {
            tool: BuildTool::Maven,
            program: PathBuf::from("mvn"),
            wrapper: false,
        };
        assert_eq!(
            toolchain.arguments(&["dependency:tree", "-Dverbose"]),
            vec!["-B", "dependency:tree", "-Dverbose"]
        );
    }

    #[test]
    fn il_catalogo_e_coerente() {
        // Due plugin con lo stesso prefisso renderebbero ambigue le voci.
        let mut prefissi: Vec<&str> = MAVEN_PLUGINS.iter().map(|p| p.prefix).collect();
        prefissi.sort_unstable();
        let mut unici = prefissi.clone();
        unici.dedup();
        assert_eq!(prefissi, unici);
        for plugin in MAVEN_PLUGINS {
            assert!(!plugin.artifact_id.is_empty());
            assert!(!plugin.prefix.is_empty());
            assert!(!plugin.goals.is_empty(), "{} non ha goal", plugin.prefix);
        }
    }

    #[test]
    fn un_comando_di_sistema_mancante_non_e_disponibile() {
        let toolchain = Toolchain {
            tool: BuildTool::Gradle,
            program: PathBuf::from("ride-gradle-che-non-esiste"),
            wrapper: false,
        };
        assert!(!toolchain.available());
    }
}
