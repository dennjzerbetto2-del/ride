# Ride

Un editor di codice multi-file con gestione di progetti, scritto in Rust con
[`egui`/`eframe`](https://github.com/emilk/egui).

Ride nasce come **scheletro già funzionante**: si apre, si scrive, si compila e
si testa. Quello che manca (il client LSP vero, qualche rifinitura) è
deliberatamente rimandato ed è elencato in fondo.

```
┌──────────────────────────────────────────────────────────┐
│ menu                                                      │
│ tab del progetto                                          │
├───────────┬──────────────────────────────┬───────────────┤
│ albero    │ editor (numeri di riga +     │ cerca nel     │
│ file      │ evidenziazione della sintassi)│ progetto     │
├───────────┴──────────────────────────────┴───────────────┤
│ barra di stato                                            │
└──────────────────────────────────────────────────────────┘
```

## Cosa sa fare

- **Progetto** – apri una cartella come progetto; l'albero dei file è pigro (i
  figli si leggono solo quando espandi una cartella) e rispetta `.gitignore` /
  `.ignore` (stesso crate di ripgrep).
- **Più progetti insieme** – *Apri cartella…* aggiunge un progetto invece di
  sostituire quello in corso; in cima all'albero compaiono le linguette dei
  progetti aperti e un clic su una di esse passa a quel progetto. Ogni progetto
  può avere un **colore personalizzato** (selezionabile dal campione accanto
  alla linguetta, salvato nella sessione), ha **le sue tab** (i file aperti di
  quel progetto, con la tab in primo piano che si ricorda), il suo albero
  (cartelle espanse e selezione si ritrovano tornando), la sua ricerca, la sua
  build e il suo language server. Aprire un file di un altro progetto ci porta
  automaticamente.
- **Sessione** – uscendo, i progetti aperti con i loro file e i loro alberi
  restano scritti: al prossimo avvio si ritrova la stessa scrivania — cartelle
  aperte, voce selezionata e progetto attivo compresi.
- **Tab** – più file aperti insieme, riordino con trascinamento, chiusura con il
  tasto centrale, `●` sulle tab modificate. Chiudere un progetto chiude le sue
  tab (chiedendo prima conferma se qualcuna è modificata).
- **Editing** – numeri di riga fissi, evidenziazione della sintassi (`syntect`),
  scorrimento orizzontale senza a capo automatico, **chiusura automatica di
  parentesi e graffe** e **apertura del blocco** con `Invio`, **evidenziazione
  della coppia di parentesi** sotto il cursore (in rosso se non è bilanciata),
  **piegatura dei blocchi `{ … }`** cliccando il marcatore `▾`/`▸` nel canale dei
  numeri di riga, indentazione automatica dopo `Invio` e `Tab`,
  commenta/indenta/duplica/sposta/cancella righe.
- **Trova e sostituisci** nel file (`Ctrl+F` / `Ctrl+H`) e **ricerca in tutti i
  file del progetto** (`Ctrl+Shift+F`, o dal menu *Visualizza*, su thread
  separato). Per impostazione predefinita la ricerca segue le stesse regole
  dell'albero (`.gitignore`, file nascosti); i due interruttori nel pannello
  (*Nascosti*, *Rispetta .gitignore*) la estendono fino a coprire ogni file.
- **Apertura rapida** file (`Ctrl+P`, filtro fuzzy) e **vai alla riga**
  (`Ctrl+G`).
- **Formattazione** con i formatter esterni del linguaggio (`rustfmt`,
  `prettier`, …): girano in background, lo strumento viene invocato via
  stdin/stdout. Se lo strumento esterno non è installato, si usa il **language
  server** (`textDocument/formatting`): è così che i file Java si formattano con
  `jdtls` **senza** dover installare `google-java-format`.
- **Language server (LSP)** per Rust, Java, Go, TypeScript/JavaScript e Python:
  diagnostica sottolineata sotto il codice, **popup *Problemi* che compare da
  solo quando arriva un errore nuovo** (con salto alla riga), conteggio nella
  barra di stato, **vai alla definizione** (`F12`), **formattazione** e pannello
  *Aiuto → Language server*. Il client parla JSON-RPC su stdio, su thread
  dedicati.
- **Comandi di build Maven / Gradle** – nei progetti Java il menu *Esegui*
  compare da solo (`pom.xml` o `build.gradle{,kts}`) e propone i comandi usuali
  (compila, test, pacchetto, …). Viene usato il *wrapper* del progetto
  (`mvnw`/`gradlew`) quando c'è, altrimenti il comando di sistema; l'output
  compare in una console che si può interrompere.
- **Goal dei plugin Maven** – sotto *Esegui → Maven → Plugin* compaiono i plugin
  che il `pom.xml` dichiara (`spring-boot:run`, `dependency:tree`, …) con i loro
  goal; per tutto il resto c'è *Comando…*, che accetta qualunque goal con le sue
  opzioni (`dependency:tree -Dverbose`).
- **Preferenze** persistenti tra le esecuzioni (tema, dimensione del font,
  pannelli, evidenziazione, LSP…).
- **Trascina un file o una cartella** sulla finestra per aprirlo.

## Come si costruisce

Serve Rust **1.85 o superiore** (edition 2024).

```bash
# eseguire
cargo run -p ride -- [percorso]…

# una cartella viene aperta come progetto (più cartelle = più progetti
# aperti insieme), un file come documento
cargo run -p ride -- src
cargo run -p ride -- crates/ride/src/main.rs

# test e controllo statico
cargo test --workspace
cargo clippy --workspace --all-targets
```

### Dialoghi nativi

Di default è attiva la feature `native-dialogs` (`rfd`): *Apri file* e *Apri
cartella* usano le finestre di dialogo del sistema. Se il sistema non offre i
portali (o vuoi zero dipendenze), compila senza:

```bash
cargo run -p ride --no-default-features
```

In quel caso al posto delle finestre native compare una finestra interna in cui
si scrive il percorso a mano.

### Se la grafica non parte

`eframe` usa il backend **wgpu** (Vulkan/Metal/DX12) di default. Su macchine
senza driver adeguati si può passare a OpenGL:

```toml
eframe = { version = "0.37", default-features = false, features = ["glow", "persistence"] }
```

## Architettura

Workspace Cargo con due crate, per tenere la **logica** separata dalla
**grafica**:

| Crate        | Cosa contiene                                                                            |
| ------------ | ---------------------------------------------------------------------------------------- |
| `ride-core`  | Il modello, senza GUI: `Document`, `Workspace`, `Project`, ricerca, editing, sintassi, formattazione, riconoscimento del sistema di build. Testabile senza aprire una finestra. |
| `ride`       | L'applicazione `egui`: stato dell'interfaccia, disegno, integrazione con `eframe`.       |

La regola che tiene in piedi tutto è una sola: **i moduli di `ui` non toccano
mai il modello**. Disegnano a partire dallo stato e restituiscono un elenco di
[`Action`](crates/ride/src/action.rs); l'unico punto che le esegue è
`RideApp::dispatch`. Così l'ordine con cui `egui` disegna i pannelli non può
produrre stati incoerenti, e ogni comando è definito una volta sola.

```
crates/
├── ride-core/src/
│   ├── document.rs   un file aperto: testo, EOL, "modificato", salvataggio
│   ├── workspace.rs  i progetti, le loro tab e il documento attivo
│   ├── session.rs    com'era la scrivania (progetti, file, alberi) da ritrovare
│   ├── project.rs    la cartella di progetto e l'albero dei file (pigro)
│   ├── search.rs     ricerca testuale/regolare nel progetto
│   ├── editor.rs     operazioni per righe (indenta, commenta, duplica…) e parentesi
│   ├── folds.rs      i blocchi `{ … }` che si possono piegare
│   ├── syntax.rs     linguaggio dedotto dall'estensione
│   ├── format.rs     formatter esterni via stdin/stdout
│   └── build.rs      riconosce e invoca Maven/Gradle
└── ride/src/
    ├── main.rs       crea la finestra e delega a `app`
    ├── app.rs        il cuore: stato + `dispatch` delle azioni
    ├── action.rs     l'elenco dei comandi e le scorciatoie
    ├── state.rs      lo stato dell'interfaccia (albero, ricerca, dialoghi…)
    ├── prefs.rs      preferenze e applicazione dello stile di egui
    ├── highlight.rs  tokenizzazione syntect + cache delle "galley"
    ├── lsp/          client LSP: JSON-RPC su stdio, diagnostica, definizioni, formattazione
    └── ui/           un modulo per ogni zona dello schermo
        ├── menu.rs   barra dei menu
        ├── tabs.rs   barra delle tab
        ├── tree.rs   albero del progetto
        ├── editor.rs numeri di riga, testo, barra trova/sostituisci
        ├── search.rs pannello "cerca nel progetto"
        ├── status.rs barra di stato
        └── dialogs.rs finestre sovrapposte
```

### Scelte tecniche

- **Niente a capo automatico.** Il `layouter` di `TextEdit` è nostro: decide i
  colori, impone `wrap.max_width = ∞` e fissa l'altezza di riga, così i numeri
  di riga restano allineati al pixel con il testo.
- **Cache dell'evidenziazione.** La "galley" (testo impaginato e colorato) è
  costosa; viene ricostruita solo quando cambia il testo, il tema o la ricerca
  attiva (chiave `HighlightKey`), altrimenti si riusa un `Arc` già pronto. Le
  galley puntano all'**atlante dei font** di `egui`: quando `epaint` lo
  ricostruisce (succede quando cambiano le opzioni di testo, fra cui la
  conversione dei colori legata al tema chiaro/scuro, o quando l'atlante si
  riempie) la cache viene buttata — altrimenti i caratteri verrebbero disegnati
  con i glifi sbagliati.
- **Solo temi di sintassi leggibili sul tema scelto.** I temi `syntect`
  disponibili sono divisi in chiari e scuri guardando il loro sfondo, e la voce
  *Tema sintassi* propone solo quelli che si accordano con lo sfondo
  dell'editor; cambiando tema chiaro/scuro il tema di sintassi corrente si
  tiene se va ancora bene, altrimenti si passa al predefinito. Serve a evitare
  il caso che rendeva il codice invisibile: un tema chiaro su fondo scuro (o
  viceversa).
- **Numeri di riga dipinti dopo lo scorrimento.** Il canale dei numeri è fissato
  a sinistra e vengono disegnate solo le righe visibili: con file enormi non
  costa nulla.
- **Piegatura per compressione delle righe.** Il testo di `TextEdit` resta
  intatto: le righe di un blocco piegato restano nella galley con gli stessi
  caratteri (i cursori e gli offset LSP non cambiano) ma con altezza zero e mesh
  svuotata, e le righe successive risalgono. Così ricerca, diagnostica e azioni
  continuano a lavorare sugli indici del documento senza conversioni.
- **Le operazioni lente non stanno nel thread dell'interfaccia.** Ricerca nel
  progetto, formattazione, elenco file, comandi di build e lettura dal language
  server girano su thread dedicati e comunicano con canali `mpsc`; il frame
  successivo raccoglie i risultati pronti. I thread del server risvegliano la
  GUI con `Context::request_repaint`.
- **Build interrompibile senza `libc`.** Maven e Gradle vengono lanciati con
  `stdout` e `stderr` su pipe letti da due thread (un solo lettore si
  bloccherebbe sul primo flusso); l'attesa è un `try_wait` in polling che
  controlla una `AtomicBool`, così il pulsante *Interrompi* uccide il processo
  senza dipendenze di piattaforma.
- **I plugin Maven si leggono dal `pom.xml` senza parser XML.** Si cercano gli
  `artifactId` *noti al catalogo* in tutto il file: un plugin in
  `<pluginManagement>` o dentro un profilo conta come dichiarato, e un artefatto
  che comincia come un plugin noto (`maven-help-plugin-extra`) no. Si propongono
  solo i plugin del progetto più i tre che Maven risolve da sé (`dependency`,
  `help`, `versions`): offrire `spring-boot:run` a chi non usa Spring Boot
  sarebbe una voce di menu che fallisce.
- **Posizioni LSP in UTF-16, disegno in caratteri.** Il protocollo conta le
  colonne in unità UTF-16; `egui` in caratteri. La conversione (`lsp::char_offset`)
  evita sottolineature spostate appena compare un carattere non ASCII.
- **Formattazione via LSP = elenco di `TextEdit`.** Il server non rimanda il
  documento intero ma un elenco di sostituzioni, ciascuna con un intervallo in
  unità UTF-16; si convertono in indici di carattere e si applicano
  **dall'ultima alla prima**, così le posizioni precedenti restano valide (a
  parità di posizione vale l'ordine dell'elenco). Vale per tutti i server che
  implementano `textDocument/formatting`.
- **Preferenze senza `serde`.** Bastano chiavi `ride.*` su `eframe::Storage`.

## Scorciatoie

L'elenco completo è anche dentro l'editor (**F1**), ed è la stessa tabella usata
per generare questa parte.

| Tasti              | Comando                          |
| ------------------ | -------------------------------- |
| `Ctrl+N` / `Ctrl+O` | Nuovo file / Apri file…         |
| `Ctrl+Shift+O`     | Apri cartella come progetto…     |
| `Ctrl+S`           | Salva                            |
| `Ctrl+Shift+S`     | Salva con nome…                  |
| `Ctrl+Alt+S`       | Salva tutto                      |
| `Ctrl+R`           | Ricarica dal disco               |
| `Ctrl+W`           | Chiudi la tab                    |
| `Ctrl+Shift+W`     | Chiudi tutte le tab              |
| `Ctrl+Tab`         | Tab successiva/precedente        |
| `Ctrl+P`           | Apri un file del progetto        |
| `Ctrl+G`           | Vai alla riga…                   |
| `Ctrl+Shift+E`     | Mostra il file attivo nell'albero |
| `Ctrl+F` / `Ctrl+H` | Trova / Trova e sostituisci      |
| `F3` / `Shift+F3`  | Corrispondenza successiva/precedente |
| `Ctrl+Shift+F`     | Cerca nel progetto               |
| `Ctrl+/`           | Commenta o scommenta le righe    |
| `Ctrl+]` / `Ctrl+[` | Indenta / disindenta            |
| `Ctrl+Shift+D`     | Duplica la riga                  |
| `Alt+↑` / `Alt+↓`  | Sposta la riga                   |
| `Ctrl+Shift+K`     | Cancella la riga                 |
| `Ctrl+Shift+I`     | Formatta il documento            |
| `F12`              | Vai alla definizione (LSP)       |
| `Ctrl+B`           | Mostra/nascondi il progetto      |
| `F5`               | Ricarica l'albero                |
| `Ctrl+ +` / `Ctrl+-` / `Ctrl+0` | Dimensione del testo |
| `F1`               | Scorciatoie                      |
| `Esc`              | Chiudi ricerca o dialogo         |
| `Ctrl+Q`           | Esci                             |

## Stato e limiti noti

Questo è uno **scheletro funzionante**, non un editor completo. In particolare:

- **LSP: un server alla volta.** Il client avvia il server del linguaggio del
  documento in primo piano e lo riavvia passando a un linguaggio diverso o a un
  progetto diverso (il server è legato a una cartella: la sua radice è quella del
  progetto attivo, e riceve i documenti di *quel* progetto). I
  server usati sono `rust-analyzer`, `jdtls`, `gopls`,
  `typescript-language-server` e `pyright-langserver`: se non sono nel `PATH`
  l'editor lo dice e la ricerca delle funzioni LSP resta spenta. La
  sincronizzazione è *full* (a ogni modifica si manda tutto il testo) e le
  funzioni coprono diagnostica, *vai alla definizione* e *formattazione*;
  completamento e hover non sono ancora collegati all'interfaccia. Il popup
  *Problemi* non è modale (non blocca le scorciatoie) e si riapre solo quando i
  problemi cambiano: una volta chiuso, lo stesso insieme non lo fa ricomparire.
- **Evidenziazione con file enormi.** Oltre 30.000 righe i colori vengono
  disattivati (il testo resta editabile): il costo per battuta crescerebbe
  troppo.
- **Formattazione** dipende da strumenti esterni installati nel `PATH`, oppure
  dal language server (che deve essere attivo e pronto). `pyright` non
  implementa `textDocument/formatting`: per Python serve `black`.
- **Comandi di build**: il menu *Esegui* compare solo per Maven e Gradle (non per
  altri sistemi di build) e propone un elenco fisso di comandi; il wrapper del
  progetto ha la precedenza sul comando di sistema. La build legge i file su
  disco: le modifiche non salvate non ne fanno parte (la console lo ricorda).
  I plugin di Maven si riconoscono dal `pom.xml` e il catalogo dei loro goal è
  scelto a mano (non tutti i plugin esistenti); per Gradle non si possono
  elencare i task senza eseguire `gradlew tasks`, quindi restano i comandi fissi
  e *Comando…*. *Interrompi* uccide il processo avviato, non l'intero albero dei
  processi figli, e l'output della console resta in memoria solo per le ultime
  righe.
- **Indentazione automatica** copre i casi comuni (indentazione della riga
  precedente + un livello dopo `{`, `(`, `[`), non è un parser completo.
- **Parentesi**: la chiusura automatica, l'apertura del blocco con `Invio` e
  l'evidenziazione della coppia sono "ingenue": non distinguono stringhe e
  commenti (una graffa in un letterale viene contata). Si possono spegnere da
  *Visualizza* (*Chiudi le parentesi automaticamente*, *Evidenzia la coppia di
  parentesi*).
- **Piegatura dei blocchi**: il conteggio delle graffe è lo stesso ingenuo delle
  parentesi, quindi in un letterale può nascere un blocco di troppo. Le righe
  nascoste restano nel testo (gli offset non cambiano) e i blocchi si riaprono
  ai primi caratteri digitati — una modifica riapre tutto, così la piegatura non
  può riferirsi a righe che non esistono più. Il marcatore compare solo nel
  canale dei numeri di riga (con più di 200.000 righe, dove i numeri spariscono,
  non c'è piegatura) e si può disattivare da *Visualizza* (*Piega i blocchi
  dalle graffe*).
- **Più progetti**: ogni progetto ha le sue tab, quindi non si possono vedere i
  file di due progetti nella stessa barra. Aprire una cartella porta con sé i
  documenti aperti quando non c'era ancora nessun progetto; chiudere un progetto
  chiude le sue tab (con conferma per quelle modificate). Il language server ne
  segue uno solo: passando a un altro progetto riparte da zero, quindi il suo
  indice va ricostruito.
- **Sessione**: alla chiusura si scrivono le cartelle aperte con i loro file e
  le loro cartelle espanse (e quale era la voce scelta); al prossimo avvio si
  riaprono da sole. I percorsi che non esistono più vengono saltati con un
  avviso, e i documenti nuovi mai salvati non tornano (non avrebbero dove). Una
  cartella si riapre solo se lo era anche il suo genitore: se l'avevi chiusa con
  dentro una sottocartella aperta, resta chiusa. Se sulla riga di comando c'è una
  cartella o un file, comanda quella: la sessione resta da parte.

## Licenza

MIT OR Apache-2.0.
