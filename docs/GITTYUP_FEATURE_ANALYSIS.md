# GittyUp → grit: Funktionsanalyse und Implementierungsplan

Stand: 3. November 2026. Ziel: native Rust-Implementierung mit egui und libgit2; vertraute GittyUp-Arbeitsabläufe. Dies ist eine überprüfbare Bestandsaufnahme, keine Behauptung vollständiger Funktionsgleichheit.

## Referenz und Methode

Analysiert wurde der öffentlich zugängliche GittyUp-Quellcode auf `master`, Commit **0115d943a8692fee20642060659ddc668904bcd9**. GittyUp wurde für diese Analyse nicht lokal gebaut oder gestartet; die Aussagen stammen aus den verlinkten Dokumenten und aktiven Quellcodepfaden. Eine lokale, nicht mitgelieferte Referenzkopie liegt unter `/tmp/grit-gittyup-reference`. Die Website beschreibt zusätzlich das Release 2.0.0 und ältere Funktionen; `master` kann darüber hinausgehende Funktionen enthalten. Implementierungen werden eigenständig in Rust geschrieben; C++/Qt-Code wird nicht übernommen.

Quellen:

- [Offizielle Funktionsübersicht und Changelog](https://murmele.github.io/Gittyup/)
- [Fixierter Quellcode](https://github.com/Murmele/Gittyup/tree/0115d943a8692fee20642060659ddc668904bcd9)
- [Menüs und globale Aktionen](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/ui/MenuBar.cpp)
- [Verlaufsdarstellung und Kontextaktionen](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/ui/CommitList.cpp)
- [Verlaufsoptionen](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/ui/CommitToolBar.cpp)
- [Strukturierte Suche](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/ui/AdvancedSearchWidget.cpp)
- [Dateiaktionen](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/ui/FileContextMenu.cpp)
- [Diff-Optionen](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/dialogs/DiffPanel.cpp)
- [Vorlagenverwaltung](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/ui/TemplateDialog.cpp)
- [Commit-Eigenschaften beim Amend](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/dialogs/AmendDialog.cpp)
- [Tag-Dialog](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/dialogs/TagDialog.cpp)
- [Repository-Konfiguration einschließlich LFS](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/dialogs/ConfigDialog.cpp)
- [Externe Werkzeuge](https://github.com/Murmele/Gittyup/blob/0115d943a8692fee20642060659ddc668904bcd9/src/dialogs/ExternalToolsDialog.cpp)
- [Hosting-Adapter](https://github.com/Murmele/Gittyup/tree/0115d943a8692fee20642060659ddc668904bcd9/src/host), [Plugins](https://github.com/Murmele/Gittyup/tree/0115d943a8692fee20642060659ddc668904bcd9/src/plugins), [Dateiwächter](https://github.com/Murmele/Gittyup/tree/0115d943a8692fee20642060659ddc668904bcd9/src/watcher).

Abgleich mit dem vorhandenen grit-Code: `src/main.rs`, `src/ui.rs`, `src/tests.rs`, README und Linux-Packaging. Ausgangszustand war der bereits lokal geänderte UI-Stand, nicht nur der letzte Git-Commit `527c646`. »Vorhanden« bezeichnet den Ausgangszustand; »Paket« bezeichnet konkrete Ergänzungen dieser Arbeitsrunde. Ein Paket gilt erst nach Integration und Tests als fertig. Dateinamen im Plan sind Implementierungsorte, keine simulierten Funktionen.

## Funktionsmatrix

### Repository und Arbeitsfläche

| Funktion in GittyUp | grit am Anfang | Umsetzung / Abnahme |
|---|---|---|
| Lokale Repositories öffnen, initialisieren, klonen | Vorhanden | Weiterverwenden; Fehler müssen sichtbar bleiben |
| Mehrere Repositories, Tabs, zuletzt verwendet | Vorhanden | Tabs und aktuelles Repo sind gespeichert |
| Repository über `.git` öffnen | Über libgit2 möglich | Worktree- und Bare-Fälle gesondert testen |
| Repository-Chooser und neue Fenster | Teilweise | Recent-Liste vorhanden; Runde 3: neues Fenster mit eigenem Repositoryprozess |
| Diff- und History-Bereich maximieren | Fehlt | `Ctrl/Cmd+M`, temporärer Fokusmodus |
| Baum-/Listen-/Doppelbaum-Dateiansicht | Staging-Doppelliste vorhanden | Echte Verzeichnisbäume für Files, Commit und Changes in Runde 2 integriert |
| Log mit Operationen und Fehlern | Nur aktuelle Statusmeldung | Begrenzte Sitzungshistorie mit vollständiger Ausgabe ergänzen |
| Dateisystemänderungen automatisch erkennen | Fehlt | Runde 3: Hintergrund-Polling alle 2 s für Arbeitsbaum/Index/Referenzen; pausierbar |
| Terminal und Dateimanager öffnen | Fehlt | Plattformabhängige explizite Benutzeraktionen; keine Shell-Interpolation |
| Sprache, Fonts, Theme, Hotkeys konfigurierbar | Dunkles Theme, wenige Shortcuts | Runde 3: System/Dark/Light, Textgröße, Fonts, eigene Datei und vier konfigurierbare globale Shortcuts; Übersetzung teilweise |
| Single-instance, Auto-Update, Plugins | Fehlt | Runde 3: Unix-Single-Instance-Weiterleitung und explizite neue Fenster; Updater/Plugins offen |

### Verlauf, Referenzen und Suche

Quelle: `CommitList.cpp`, `CommitToolBar.cpp`, `AdvancedSearchWidget.cpp`, `History.cpp`, `PathspecWidget.cpp`.

| Funktion | grit am Anfang | Umsetzung / Abnahme |
|---|---|---|
| Mehrbranch-Graph, Autor/Datum/SHA, Referenz-Badges | Vorhanden | Bestehende Graph-Regression beibehalten |
| Alle oder ausgewählte Branches anzeigen | Vorhanden | Ancestry-Filter vorhanden |
| Ausgewählter Branch nur über ersten Parent | Fehlt | Elternkette tatsächlich beschränken; Merge-Seitenzweige ausschließen |
| Nach Datum oder topologisch sortieren | Nur topologisch mit Zeitanteil | Separate Anzeigeoptionen mit stabilen Graph-Lanes |
| Graph/Autor/Datum/SHA ein-/ausblenden, kompakte Zeilen | Fehlt | Anzeigeoptionen ergänzen |
| Commits markieren (Sterne), nur markierte ansehen | Fehlt | Persistente, repositorybezogene Commit-IDs; Kontextaktion und Filter |
| Navigation per Tastatur | Fehlt | Auf/Ab/Home/End bei fokussierter Tabelle, kein Abfangen beim Texteingeben |
| Verlauf vor/zurück | Fehlt | Eigene Auswahlhistorie, getrennt von Git-Reflog |
| Nachricht und Autor suchen | Vorhanden | SHA und E-Mail zusätzlich durchsuchen |
| Felder `author`, `email`, `message`, `date`, `after`, `before` | Fehlt | Validierte strukturierte Filter; ISO-Datumsgrenzen inklusiv |
| Dateiname/Pfad im Commit suchen | Fehlt | Exakter Dateipfadfilter ohne Shell-/Pathspec-Interpretation |
| Diff-Kontext, Ergänzungen, Löschungen, Hunk-Scope durchsuchen | Fehlt | Runde 3: eigener asynchroner Diffindex mit Addition/Deletion/Context/Hunk-Feldern |
| Kommentare, String-Literale, Identifier durchsuchen | Fehlt | Runde 3: begrenzter Rust-Index mit explizit heuristischen Lexikfeldern |
| Suchvorschläge, Query-Sprache mit Gruppierung | Fehlt | Runde 3: eigene AND/OR/NOT-Klammergrammatik, Quotes und Feldvorschläge |
| Commit komplett lesen, Autor und Committer unterscheiden | Nur Summary und Autor | Vollständige Message/E-Mail/Committer/Eltern ergänzen |
| Dateiversionen/Dateiverlauf und vorherige/nächste Revision | Nur aktueller Text/Blame | Luna-Dateiversionspaket |
| Blame mit integrierter Timeline | Nur `git blame`-Text | Interaktive Attribution/Commitliste in Runde 2; Heatmap bleibt offen |

### Änderungen, Diff und Staging

Quelle: `DiffView/HunkWidget.cpp`, `DiffView/Images.cpp`, `FileContextMenu.cpp`, `DoubleTreeWidget.cpp`, `DiffPanel.cpp`.

| Funktion | grit am Anfang | Umsetzung / Abnahme |
|---|---|---|
| Getrennte vorgemerkte/nicht vorgemerkte Änderungen | Vorhanden | Gleiche Datei auf beiden Seiten korrekt getrennt |
| Datei/alle Dateien stagen und unstagen | Vorhanden | Tatsächliche Indexinhalte testen |
| Einzelne Hunks stagen | Vorhanden | Gegen falschen oder veralteten Patch absichern |
| Einzelne Zeilen auswählen und stagen/unstagen | Nur manuelles Patch-Editing | Luna-Patchpaket; echte Apply-Tests für Vorwärts-/Rückwärtsrichtung |
| Tastatur S/U/R und Markierung im Diff | Fehlt | S/U für selektierte Änderungen; Discard weiter mit Bestätigung |
| Dateien/ausgewählte Änderungen verwerfen | Dateien vorhanden | Runde 3: bestätigter selektiver Reverse-Patch im Worktree mit Snapshot-Prüfung |
| Ungetrackte Dateien ausblenden | Fehlt | Anzeigepräferenz; Daten nicht verändern |
| Ignore für einzelne Dateien und Muster | Fehlt | Luna-Repositorypaket; escaping, bestehende `.gitignore`, Symlinks testen |
| Alte/neue Zeilennummern und farbige Herkunft | Vorhanden | Diff-Origin-Regression vorhanden |
| Dynamischer Zeilenumbruch | Fehlt | Renderoption, Patch-Rohdaten unverändert |
| Kontextanzahl, Whitespace ignorieren, Encoding auswählen | Fehlt | Kontext/Whitespace Runde 2; UTF-8, Latin-1, Windows-1252, UTF-16LE/BE mit strikter Vorschau Runde 3 |
| Bild-/Binärvorschau, gelöschte Bilder | Fehlt | Runde 3: begrenzte Old/New-Bildvorschau auch für gelöschte historische Bilder |
| Große Diffs erst nach Nachfrage laden | Fehlt | Größen-/Zeilenlimit und explizites »Load« ergänzen |
| Dateiversion exportieren/mit Anwendung öffnen | Fehlt | Luna-Dateiversionspaket, bytegetreuer Export, Überschreibbestätigung |
| Dateipfad kopieren, Dateiverlauf filtern, Executable-Bit ändern | Fehlt | Copy path/History vorhanden; Runde 3: expliziter Index-Executable-Schalter |
| Externer Editor/Diff/Merge | Fehlt | Runde 3: Editor, externe Diff-/Merge-Werkzeuge mit argv-Vorlagen und temporären Dateiversionen |

### Commits, Branches und Konflikte

Quelle: `CommitEditor.cpp`, `TemplateDialog.cpp`, `AmendDialog.cpp`, `MergeDialog.cpp`, `TagDialog.cpp`, `RepoView.cpp`.

| Funktion | grit am Anfang | Umsetzung / Abnahme |
|---|---|---|
| Normale Commits und Amend | Vorhanden | Bestehenden Workflow erhalten |
| Wiederverwendbare Commit-Vorlage und Git `commit.template` | Fehlt | Luna-Commitpaket; nicht ungefragt vorhandene Eingabe überschreiben |
| Mehrere benannte Vorlagen, Sortieren/Import/Export | Fehlt | Weiterer Ausbau nach einfacher Vorlage |
| Commit-Autor überschreiben, Sign-off | Fehlt | Luna-Commitpaket; argv, Identität und echte Commit-Metadaten testen |
| Autor/Committer-Zeitstempel gezielt ändern | Fehlt | Runde 3: eigener Amend-Dialog für separate Autor-/Committer-Identität und Zeit mit Offset |
| Branch erstellen, sicher löschen, Checkout | Vorhanden | Keine erzwungenen Löschungen |
| Branch umbenennen | Fehlt | Luna-Repositorypaket |
| Branch mergen; Konflikte fortführen/abbrechen | Rebase vorhanden, Merge-Start fehlt | Luna-Repositorypaket plus Merge-Abschluss in Changes |
| Rebase, interaktives Rebase, Squash/Fixup/Drop | Vorhanden als Planeditor | Runde 3: grafischer Plan mit Aktionen/Up/Down/Drop; Raw-Editor bleibt verfügbar |
| Cherry-pick/Revert und deren Konfliktfortsetzung | Start vorhanden | Fortsetzen/Abbrechen zustandsabhängig ergänzen |
| Detached Checkout / Branch an Commit | Vorhanden | Weiterverwenden |
| Soft/Mixed/Hard Reset | Fehlt | In Runde 3 integriert: Ziel-OID, Modus und Folgen prüfen; Hard verlangt zusätzliche Bestätigung |
| Leichte und annotierte Tags, Löschen | Nur leichtes Tag | Annotieren/Löschen im Luna-Repositorypaket |
| Tag aus bestehendem Namen ableiten; Tag pushen/ersetzen | Fehlt | Tagliste als Vorlage; Force bleibt gesonderter Ausbau |
| Stash speichern, anwenden, poppen, löschen | Vorhanden | Runde 3: Namen, untracked/keep-index/staged-only und optionale Index-Wiederherstellung |
| Konflikte Dateiweise ours/theirs/manuell lösen | Vorhanden | Binärspezifische Vorschau fehlt; kein Auto-Abbruch nach Konflikt |

### Remotes, Hosting und größere Repositories

Quelle: `RemoteDialog.cpp`, `ConfigDialog.cpp`, `UpdateSubmodulesDialog.cpp`, `RemoteCallbacks.cpp`, `host/*`, `cred/*`.

| Funktion | grit am Anfang | Umsetzung / Abnahme |
|---|---|---|
| Remotes verwalten, fetch, pull, push, upstream veröffentlichen | Vorhanden | Pull derzeit ausdrücklich nur fast-forward |
| Bestimmten Remote/Branch wählen, Tracking konfigurieren | Teilweise | Runde 3: gezielter Branchpush und Upstream-Auswahl |
| Automatisches Fetch und Prune-Option | Prune bei manuellem Fetch | Runde 3: pro Repository gespeichertes Opt-in mit Intervall und Prune |
| Push force-with-lease / Tags | Fehlt | Runde 3: bestätigte Tags und Force-with-lease mit gepinnter Quelle/Ziel/Erwartungs-OID |
| Git-Credential-Helper und SSH-Agent | System-Git vorhanden | Kein eigener unsicherer Passwortspeicher |
| Hosting-Konten: GitHub/GitLab/Gitea/Bitbucket/Beanstalk | Fehlt | GittyUp besitzt konkrete Adapter; API-Featureparität offen |
| Pull-Request-Dialog und Hosting-Aktionen | Fehlt | Runde 3: geprüfte Browser-Workflows für PR/MR/Code-Review; native Accounts/API weiterhin offen |
| Submodule ansehen, initialisieren, aktualisieren, öffnen | Fehlt | Luna-Repositorypaket; rekursive Updates explizit |
| Git LFS initialisieren, Tracking, Locks, Settings | Fehlt | Eigenes Paket; `git-lfs` muss vorhanden sein; nicht als allgemeine Git-Funktion vortäuschen |
| Worktrees und Bare-Repositories | libgit2 zum Teil vorhanden | Runde 3: Worktree-Verwaltung mit Create/Open/Remove/Prune; Bare-History geprüft |
| SHA-256-Repositories | Nicht verifiziert | Explizite Erkennung und verständliche Ablehnung; native libgit2-Unterstützung bleibt offen |
| Asynchroner Suchindex, Lexer, Dateiwächter | Fehlt | Architekturpaket, getrennt von GUI-Neugestaltung |

## Konkrete Agentenpakete

| Paket | Verantwortlich | Dateien | Abnahme |
|---|---|---|---|
| A: Zeilenweises Stage/Unstage | Luna `diff_line_staging` | `src/diff_features.rs` | Mehrere Hunks; selektive Add/Delete-Zeilen; CRLF; EOF; neue/gelöschte Dateien; tatsächlicher Index danach |
| B: Repository-Werkzeuge | Luna `repository_features` | `src/repository_features.rs` | Rename/Merge/Tags gegen echte Temp-Repos; Ignore literal und Muster; Submodule-Zustände |
| C: Commit-Vorlagen/Autor | Luna `commit_templates` | `src/commit_features.rs` | Template-Pfade, Überschreibschutz, reale normale/amend Commits, Autor und Sign-off |
| D: Dateiversionen | Luna `commit_templates`, Folgepaket | `src/file_features.rs` | Revisionen inkl. Root/Delete, bytegetreue Exporte, kein Symlink-Überschreiben |
| E: History und Integration | Hauptagent | `src/history_features.rs`, `src/main.rs`, `src/ui.rs` | Sterne/Persistenz, Filter, erster Parent, Tastaturnavigation, Bedienbarkeit |
| F: Abgleich und Release | Hauptagent | dieses Dokument, README, Packaging | Tests/Clippy, echte UI-Klicktests, AppImage-Build-ID und direkter AppImage-Start |

Die Subagenten bearbeiten ausschließlich ihre Module. Der Hauptagent prüft den Code, verkabelt Bedienoberfläche und Persistenz und bewertet Testergebnisse. Ein grüner isolierter Unit-Test genügt nicht für die Freigabe eines Features, das den Index verändert.

## Reihenfolge und weitere Arbeit

1. Kernbedienung schließen: Zeilen-Staging, Filter/Sterne, Vorlagen, Rename/Merge/Tags, Ignore, Dateiversionen.
2. Repository-Betrieb: Fortsetzen/Abbrechen aller Konfliktzustände, Operationenlog, Submodule, lokale Identität.
3. Diff/Dateikomfort: Kontext/Whitespace/Wrap, große Diffs, externe Werkzeuge, Bildvorschau, interaktives Blame.
4. Netzwerk: gezielte Remotes/Tracking, Tag-Push und optionales Auto-Fetch; anschließend LFS.
5. Spezialisierte Gleichheit: Suchindex mit Codefeldern, Hosting-Accounts/PRs, Vorlagenbibliothek, Themes/Sprachen/Hotkeys, Updater/Plugins.

Offene Funktionen bleiben ausdrücklich als offen markiert. Insbesondere sind ein flaches Suchfeld kein Code-Suchindex, ein System-SSH-Aufruf keine Hosting-Kontenverwaltung und ein textueller Blame-Output keine interaktive Timeline.

## Integrations- und Testprotokoll

Ausgangspunkt waren acht Backend-Regressionstests und die geprüfte AppImage nach Korrektur des veralteten AppDir-Caches. Packaging nutzt nun frische Staging-Verzeichnisse; geprüft wird das extrahierte AppImage-Binary, nicht allein `target/release/grit`.

### In dieser Runde integriert

| Feature | Implementierung | Ergebnis |
|---|---|---|
| Selektives Stage/Unstage | `diff_features.rs`, Index-Patchaktion, anklickbare Diff-Zeilen | 14 Git-basierte Patchtests; UI-Einzelzeile stagen/unstagen geprüft, Worktree-SHA blieb gleich |
| Vorlagen/Autor | `commit_features.rs`, eigenes Optionsfenster, Persistenz | Benannte Vorlagen, Add/Replace/Remove/Import/Export, Apply-Bestätigung, Git-Vorlage, Amend-Message, Author-Override, Sign-off |
| Historische Dateien | `file_features.rs`, Files-Toolbar und Dateikontext | Add/Update/Delete, Text/Binär-Erkennung, Revisionen, vorherige/nächste Version, bytegetreuer Export; ausstehender Export an Blob-ID gebunden |
| Branches/Tags/Submodule/Ignore | `repository_features.rs`, Werkzeugansicht | Rename/Merge, annotierte Tags mit Löschbestätigung, Submodule, Ignore literal und raw pattern; echte Git-Tests |
| LFS und externe Programme | `integration_features.rs`, eigene Ansicht | Verfügbarkeits-/Init-Cache, explizites lokales Init/Track/Untrack und Locks; Editor/Terminal/Ordner, Terminalerkennung inklusive Konsole |
| History-Komfort | `history_features.rs`, Verlaufstabelle | Repositorybezogene persistente Sterne, strukturierte UND-Suche mit Quotes/Datumsvalidierung, exakter Pfad, First-parent, Sortierung/Spalten/kompakt, Back/Next und Tastaturnavigation |
| Detail-/Diff-Komfort | `main.rs`, `ui.rs` | Vollständige Message, Autor-E-Mail/Committer/Eltern, Wrap, große Diffs explizit laden, versteckte untracked Dateien, Fokusmodus |
| Operationszustände | `main.rs`, `ui.rs` | Merge/Rebase/Cherry-pick/Revert fortsetzen/abbrechen; Refresh auch bei Fehlern, Sitzungslog, Refresh beim Fokuswechsel |

Die datumsbasierten Suchgrenzen sind inklusiv und berücksichtigen den Commit-Zeitzonenoffset. `path:` sucht Änderungen an exakt diesem Pfad, verfolgt Umbenennungen nicht automatisch und verwendet aktuell den ersten Parent als Vergleich. Die strukturierte Suche unterstützt keine Code-Identifier-/Addition-/Deletion-Abfragen und keine boolesche Klammergrammatik.

### Review-Korrekturen

- Nicht selektierte Additionen dürfen beim Stage nicht zu Kontext werden. Vorwärts-/Rückwärtsrichtung und neue/gelöschte Dateien haben deshalb eigene Apply-Tests.
- Diff-Zeilen können selbst mit `+++` oder `---` beginnen. Dateiköpfe werden anhand der Hunk-Position unterschieden, nicht allein anhand dieser Zeichen.
- Hunks benötigen Offsets relativ zum tatsächlichen Index. Multi-Hunk- und Multi-Datei-Tests prüfen dies.
- Dateinamen mit `[]`, `*` und Backslash benötigen literale Pfade. libgit2-Pathspec-Matching ist für eine ausgewählte Datei deaktiviert; Ignore-Escaping wird mit `git check-ignore` getestet.
- Export-Bestätigungen müssen die ursprüngliche Blob-Version behalten, auch wenn zwischenzeitlich eine andere Revision ausgewählt wird.
- LFS-Abfragen dürfen nicht pro Renderframe Prozesse starten. Cache mit explizitem Refresh/Invalidierung; kein automatischer Netzwerkzugriff.
- Externe Editoren erhalten absolute geprüfte Pfade, damit Dateinamen wie `-file.txt` keine Programmoptionen werden. Terminalprogramme werden auf dem Zielsystem erkannt.
- Vorlagen-Bestätigungszustand muss zwischen Frames erhalten bleiben. Kein stilles Ersetzen einer bestehenden Commit-Nachricht.

### Explizite Grenzen

Textzeilen-Staging unterstützt keine Binär-, Rename/Copy- oder reinen Mode-Patches; hierfür bleibt das Staging der gesamten Datei. Leere Dateien haben keine Textzeilen zum Selektieren. Nicht darstellbare partielle EOF-Patches werden von Git mit sichtbarer Fehlermeldung abgelehnt. In Runde 3 wurden begrenzter lexikalischer Suchindex, Bildvorschau, externe Diff-/Merge-Werkzeuge und bestätigte Reset-/Force-with-lease-Dialoge ergänzt. Weiter offen bleiben insbesondere Hosting-Konten/API-PR-Verwaltung, vollständiger Blame-Heatmap-/Copy-Move-Ausbau, vollständige Übersetzung, native SHA-256-Unterstützung und Updater-/Plugin-Infrastruktur. Browser-Hostingworkflows werden getrennt von Konten-/API-Funktionen bewertet.

### Commit-Regel

Auf Benutzerwunsch erhält jedes fertig geprüfte Feature einen eigenen Commit. Die Module haben getrennte Feature-Commits; die gemeinsame Verdrahtung der Oberfläche und Persistenz wird als Integrationscommit festgehalten. Dokumentations- und Packaging-Korrekturen bleiben getrennt.

### Geprüfter Zwischenstand

- `cargo test --locked`: **41 Tests bestanden**.
- `cargo clippy --locked --all-targets -- -D warnings`: erfolgreich.
- `cargo fmt --all --check` und `git diff --check`: erfolgreich.
- UI-Prüfungen in isoliertem Test-Repository: einzelne Zeile Stage/Unstage (Indexinhalt und Worktree-Prüfsumme), strukturierte Suche, Sterne über Neustart, Vorlagen über Neustart, First-parent, Fokusmodus, Dateiversionen und Ansichten bei 980 × 680.
- Eine zunächst doppelt verwendete ScrollArea-ID im Dateiverlauf wurde nach Sichtprüfung korrigiert.
- Zwischenstand-AppImage wird aus diesem integrierten Stand gebaut; offene Funktionsgruppen sind weiterhin oben dokumentiert.

## Zweite Implementierungsrunde

### Diff-Kontext und Whitespace-Anzeige

Luna-Paket `diff_preferences`: Kontextumfang 0–50 Zeilen und Whitespace-Ignorieren für Arbeitsbaum-, Index- und Commit-Diffs. Die Anzeige wird getrennt vom unveränderten Rohpatch gecacht; ein Wechsel der Anzeigeoptionen verwirft alte Zeilenmarkierungen. Bei ausgeblendetem Whitespace ist Einzelzeilen-Staging deaktiviert, während Datei-Staging weiterhin alle tatsächlichen Änderungen übernimmt. Kontextänderungen allein lassen selektives Stage/Unstage zu. Präferenzen werden gespeichert.

Abnahme: 45 Tests insgesamt grün; zusätzlicher Index-Test stagt eine Addition bei Kontext 0 und bestätigt unveränderte Arbeitsdatei sowie erhaltene übrige Änderungen. Git-Vergleiche prüfen Whitespace-only, gemischte Änderungen, staged und historische Diffs. Clippy ist sauber.

### Strukturiertes interaktives Blame

Luna-Paket `rich_blame`: Dateiannotation über libgit2 mit Autor, E-Mail, Commit, Datum, ursprünglicher und aktueller Zeilennummer. Eine Commitliste zeigt die jeweilige Anzahl zugerechneter Zeilen. Arbeitskopie und historische Revision sind getrennte Aufrufe; uncommitted Zeilen werden ausdrücklich gekennzeichnet. Klick auf eine Attribution öffnet den zugehörigen Commit und hebt eventuell verdeckende Such-/Branchfilter auf. Aufruf über Files → Blame sowie Commitdatei → Blame revision. Laden läuft im Worker; die Zeilenansicht ist virtualisiert. Refresh behält die angeforderte Revision bei.

Abnahme: 47 Tests insgesamt grün; echte Repos mit zwei Autoren prüfen historische Zuordnung, nicht committete Zeilen und Fehlerfälle. UI bei 980 × 680 geprüft, inklusive Navigation von Blame in den Commitverlauf. Ein vollständiger Blame-Heatmap-Renderer und Copy/Move-Erkennung bleiben offen.

### Hierarchische Dateiansicht

Luna-Paket `file_tree`: List-/Tree-Umschaltung für Files, Commit-Dateien und beide Changes-Listen. Verzeichnisse stehen vor Dateien und sind alphabetisch sortiert; alle Aktionen behalten den vollständigen ursprünglichen Dateipfad. Statusmarker A/M/D bleiben sichtbar. Ordner lassen sich über Pfeil oder Namen aufklappen. Die Zustands-IDs enthalten Repository, Listenart und vollständigen Pfad, damit staged/unstaged dieselben Ordner unabhängig öffnen können. Zeilen und Ordnerheader sind auf die verfügbare Breite begrenzt; lange Namen sind im Hover vollständig sichtbar. Stage/Unstage und bestätigtes Discard sind im Baum über die Auswahl-Werkzeugleiste erreichbar. Der Listenmodus bleibt erhalten; die Moduswahl wird gespeichert.

Abnahme: 51 Tests insgesamt grün, Clippy und Formatprüfung erfolgreich. Pure Baumtests prüfen verschachtelte/Unicode-/Backslash-Pfade, Dateiverzeichnis-Präfixe und stabile Reihenfolge. Sichtprüfung bei 980 × 680 sowie Klicktest für Stage/Unstage eines verschachtelten Dateipfads: Index enthielt exakt `src/core/parser.rs`, danach wieder keinen Pfad; die SHA-256-Prüfsumme der Arbeitsdatei blieb gleich.

## Dritte Implementierungsrunde

### Soft-, Mixed- und Hard-Reset

Luna-Paket `reset_workflows`: Repository Tools bietet einen Reset auf den im Verlauf ausgewählten Commit. Die Bestätigung ist an Repository, symbolischen HEAD, aktuelle HEAD-OID, Ziel und Modus gebunden. Ein Branchwechsel bei gleicher OID lässt sie verfallen. Aktive Merge-/Rebase-/Cherry-pick-Operationen sperren Reset; vorhandene lokale Änderungen bleiben entsprechend dem gewählten Modus zulässig. Hard verlangt zusätzlich die ausdrückliche Anerkennung des Datenverlusts, einschließlich ungetrackter Dateien, die einem Checkout im Weg stehen.

Abnahme: vier Tests gegen echte temporäre Repositories prüfen Soft mit verändertem Index, Mixed/Hard, symbolischen Branchwechsel und laufenden Mergezustand. Die Ziele werden zu Commits aufgelöst und als vollständige OID über argv an Git übergeben.

### Autor-/Committer-Metadaten und Zeitstempel

Luna-Paket `commit_metadata`: Im Commit-Optionsfenster öffnet „Edit HEAD metadata“ den vollständigen Committext und getrennte Autor-/Committer-Felder einschließlich RFC3339-Zeit mit Zeitzonenoffset. Review zeigt Alt/Neu. Der Worker prüft HEAD-OID, symbolische Referenz und inaktiven Operationszustand erneut. `commit --amend --only --no-edit --reset-author` übernimmt explizite Identitäten/Datumswerte und lässt vorhandene Indexänderungen aus dem Commit heraus. Der gemeinsame Operationsworker sperrt parallele Git-Aktionen.

Abnahme: Echter Git-Test über die Produktionsfunktion prüft beide Identitäten, Zeitstempel und Offsets sowie unveränderte Nachricht, Commit-Tree und vorgemerkten Index-Tree. Der Test fand und korrigierte die fehlende `--reset-author`-Option.

### Automatische lokale Änderungserkennung

Der Repository-Watcher berechnet alle zwei Sekunden im Hintergrund einen Fingerabdruck aus Referenzen, Indexeinträgen, Status und Dateimetadaten. Er erkennt auch erneute Bearbeitung bereits veränderter Dateien, neue ungetrackte Dateien sowie Branch-/Stash-/Indexänderungen. Änderungen aktualisieren die Ansichten auch ohne Fokuswechsel. Eine laufende Git-Aktion verzögert Refresh, Repositorywechsel verwerfen fremde Ergebnisse; Repository Tools bietet Pause und sichtbare Fehler. Kein Netzwerkzugriff.

Abnahme: Ein echter Repositorytest prüft stabile Fingerabdrücke sowie Worktree-Bearbeitung, erneute Bearbeitung, Index, Branch, verschachtelte untracked Datei und Löschung. Es handelt sich um portables Polling, keinen Betriebssystem-Eventwatcher. Große Repositories werden im Hintergrund gescannt; der aktuelle Refresh-Pfad wird anschließend im UI-Thread ausgeführt.

### Gezielte Remotes, Tracking, Tags und Force-with-lease

Luna-Paket `remote_workflows`: Remotes bietet Branch-/Remote-Auswahl, Upstream-Konfiguration und Tag-Push. Pushes verwenden eine aufgelöste vollständige Quell-OID und `--no-follow-tags`, damit persönliche Git-Konfiguration keine weiteren Tags ergänzt. Force-with-lease zeigt explizite Quell-/Zielreferenz und die beobachtete OID des exakt gewählten Remote-Tracking-Branches. Die Bestätigung bindet Repository, symbolischen HEAD/OID, Remote-URLs und sämtliche Ziele; Änderungen verwerfen sie. Git prüft die erwartete OID nochmals serverseitig.

Automatisches Fetch ist standardmäßig aus, wird pro Repository gespeichert und läuft nur für das aktuell geöffnete Repository nach dem gewählten Intervall. Prune ist separat wählbar. Abnahme: Zwei Tests prüfen die Produktions-Argumentbuilder und echte Tracking-/Tag-/Lease-Abläufe gegen ein lokales Bare-Remote; ein inzwischen geänderter Remote-Branch lehnt die veraltete Lease ab. Kein Test kontaktiert einen Hosting-Dienst.

### Benannte Stashes und Wiederherstellungsoptionen

Luna-Paket `stash_features`: Repository Tools → Stash options bietet Namen, explizites Include untracked, Keep index oder Staged only (Git ≥ 2.35). Ungültige Kombinationen sind gesperrt; Folgen werden vor dem Erstellen angezeigt. Apply/Pop kann den Indexzustand optional wiederherstellen. Die Auswahl ist an Repository und Stash-OID gebunden; ein verschobenes `stash@{n}` wird abgelehnt. Drop hat eine eigene OID-gebundene Bestätigung. Der Sidebar-Einstieg führt zu den gemeinsamen Werkzeugen.

Abnahme: Sieben Tests prüfen echte Stashes mit Sonderzeichen im Namen, untracked Standard/Opt-in, Keep index, Staged only, Index-Restore und OID-Änderungen. Git kann überlappende ungestagte Änderungen bei Staged only ablehnen; die UI nennt diese Grenze und zeigt Git-Fehler unverändert an.

### Externe Diff- und Merge-Werkzeuge

Luna-Paket `external_diff_features`: Integrations speichert ausführbares Programm und Argumentvorlagen (ein Argument pro Zeile). Diff ersetzt `{old}`/`{new}` durch absolute temporäre Dateipfade; Working/Index/HEAD und historische erste-Parent-Versionen sind getrennte Quellen. Merge stellt `{base}`, `{ours}`, `{theirs}` aus den unaufgelösten Indexstufen und `{merged}` als geprüfte Arbeitsdatei bereit. Fehlende Konfliktseiten sind leere Dateien. Snapshots sind schreibgeschützt, bleiben bis zum Prozessende erhalten und werden danach entfernt. Der Merge-Ausgang wird nicht automatisch gestagt.

Abnahme: Vier Tests starten ein eigenes Prüfprogramm mit Leerzeichen-Pfaden, prüfen tatsächliche Inhalte für alle Diffquellen und Cleanup. Ein echter Konflikt prüft drei Stufen und die geschriebene Arbeitsdatei, während der Index unaufgelöst bleibt. Traversal, Symlinks und ungültige Platzhalter werden abgewiesen; kein Shell-Parsing der Vorlagen.

### Bildvorschau und Textencodings

Luna-Paket `preview_features`: Changes und Commit-Diff zeigen alte/neue Bildversionen einschließlich hinzugefügter und gelöschter Bilder. Files und Dateiversionen zeigen ebenfalls eingebettete Bilder. PNG/JPEG/GIF/BMP/WebP werden durch die bereits transitiv verwendete Rust-Bildbibliothek dekodiert (GIF als statisches Bild). Quelldaten sind pro Auswahl gecacht; Textdateien lösen keinen Bild-Diff-Lesevorgang aus. Grenzen: 16 MiB Quelldaten, 16 Millionen Pixel, 64 MiB Dekodierbudget und 128 MiB/32 Einträge für Texturen. Größenprüfung erfolgt vor Blob-/Dateilesen; Quelldaten werden bei Refresh invalidiert.

Textansichten bieten UTF-8, Latin-1, Windows-1252 und UTF-16LE/BE, einschließlich BOM-Erkennung. Dekodierfehler bleiben sichtbar. Alternative Diffencodings zeigen eine schreibgeschützte Old/New-Vorschau bis 4 MiB pro Seite; selektives Textstaging steht dort nicht zur Verfügung. Rohpatches werden nicht umkodiert.

Abnahme: Drei Tests prüfen alle Encodings, ungültige UTF-Sequenzen, echte Repository-Versionen (Root/Add/Delete, Working/Index/Historie), Traversal und Bildgrößenlimits. Gesamtstand nach Integration: 84 Tests grün.

### Linked Worktrees und Bare-Repositories

Luna-Paket `worktree_features`: Repository Tools listet Worktrees anhand des NUL-getrennten Git-Porcelainformats, inklusive Leerzeichen/Unicode-Pfaden, Branch/Detached/Bare/Lock/Prunable-Status. Neue Worktrees erhalten einen neuen Branch am ausgewählten Commit oder HEAD. Öffnen wechselt tatsächlich das aktive Repository. Entfernen braucht Bestätigung und verwendet niemals Force; schmutzige oder gesperrte Worktrees verweigert Git. Prune bietet Vorschau und separate Bestätigung.

Abnahme: Vier Tests prüfen echten Create/List/Remove-Verlauf, verweigertes Entfernen bei Änderungen, Detached-Einträge sowie Bare-History und Worktreeverwaltung. SHA-256-Repositories werden über System-Git erkannt und beim Öffnen verständlich als vom libgit2-Backend nicht unterstützt gemeldet. Dies ist ausdrücklich keine SHA-256-Featureparität; dafür ist ein zusätzlicher Git-Backendpfad oder passende Bibliotheksunterstützung notwendig.

### Asynchrone Code-/Diffsuche und boolesche Query-Sprache

Luna-Paket `code_search_features`: Die separate Ansicht „Code & diff search“ baut auf ausdrücklichen Aufruf einen lokalen Index aus lokalen/Remote-Branches, Tags und detached HEAD. Suchfelder: `addition`, `deletion`, `context`, `hunk`, `comment`, `string`, `identifier`, `path`, `message`, `author`. Unterstützt werden AND/OR/NOT (Priorität NOT > AND > OR), implizites AND, Klammern, Quotes und Escapes. Feldbuttons ergänzen Query-Vorschläge. Treffer öffnen echten Commit und Datei. Index und Suche laufen in einem Worker; Repositorygeneration und Querysequenz verhindern veraltete Ergebnisse.

Grenzen werden in der UI angezeigt: maximal 2.000 Commits, 120.000 Diffzeilen, konservativ 32 MiB Textbudget und 3.000 Treffer. Merge-Diffs beziehen sich auf den ersten Parent. Lexikfelder nutzen Heuristiken für Rust/C/C++/JS/TS/Python und weitere einfache Kommentarformen, keinen vollständigen Sprachparser; unbekannter Code außerhalb eines Hunks kann den Lexikzustand beeinflussen. Alter/neuer Zustand werden getrennt pro Hunk geführt. Working tree ist nicht Bestandteil des Historyindexes.

Abnahme: Zehn Tests prüfen Parserpriorität/Klammern/Fehler/quoted Operatorliterale, tatsächliche Add/Delete/Context/Hunk-Treffer, Strings/Kommentare/Identifier, Mehrzeilenheuristik, Branch-only-Historie, unborn Repositories, große Einzelzeilen und Worker-Cancel/Query-Coalescing.

### Grafischer interaktiver Rebase-Plan

Luna-Paket `rebase_features`: Der Rebase-Dialog zeigt Commits mit Aktionsauswahl (Pick/Reword/Edit/Squash/Fixup/Drop, einschließlich Fixup -c/-C), Auf/Ab-Steuerung und Drop-Toggle. Kommentare, komplexe Git-Anweisungen und unbekannte Formen bleiben unverändert erhalten; alternativ ist weiterhin der Raw-Editor verfügbar. Die Validierung prüft erneut nach Raw-Änderungen, ob Squash/Fixup einen früheren übernommenen Commit besitzt, und sperrt Ausführen bei Fehlern.

Abnahme: Drei Tests prüfen Reorder, Aktionsänderungen, unveränderten Roundtrip inklusive Zeilenenden/Kommentaren/Advanced-Anweisungen und ungültige erste Squash/Fixup-Zeilen. Der vorhandene Git-Rebase-Worker führt den überprüften Plan aus.

### Selektives Discard und Executable-Bit

Luna-Paket `file_action_features`: Markierte ungestagte UTF-8-Diffzeilen können über „Discard selected unstaged lines“ oder R geprüft und bestätigt verworfen werden. Die Vorschau enthält den tatsächlich angewendeten Reverse-Patch. Repository, symbolischer HEAD, ursprünglicher Diff, Auswahl und Indexzustand werden vor Review und im Worker erneut geprüft. Git wendet den Patch ausschließlich im Arbeitsbaum an; der Index bleibt erhalten. Bei Whitespacefilter, alternativem Textencoding, staged Auswahl oder laufender Operation ist die Funktion nicht erreichbar.

Files → Executable bit bietet einen expliziten Schalter für den Indexmodus regulärer getrackter Dateien. Symlinks, Submodule und Konfliktstufen sind ausgeschlossen. Arbeitsdatei und Betriebssystemrechte werden nicht verändert.

Abnahme: Vier echte Git-Tests prüfen context 0/3, mehrere Hunks, gemischte Ergänzungen/Löschungen, CRLF, neue/gelöschte Dateien, unveränderten Index und erhaltene übrige Änderungen, stale Review sowie literal Dateipfade beim Moduswechsel.

### Darstellung, Schrift und Tastenkürzel

Luna-Paket `settings_features` mit Palette-Integration durch den Hauptagenten: Settings speichert System/Dark/Light, Textgröße, proportionale/Monospace-Schrift sowie eine eigene TTF-/OTF-Datei. Die zentrale UI-Palette passt Panels, Text, Auswahl und Diffhintergründe an Light/Dark an. Eigene Fonts sind auf 8 MiB begrenzt und werden vor Übergabe an egui durch `ttf-parser` geprüft. Fehler sind sichtbar; Rückkehr zu eingebauten Fonts setzt die Fontdefinitionen zurück.

Open/Refresh/Focus/Quit sind konfigurierbar; doppelte, unmodifizierte Buchstaben- und reservierte Bearbeitungs-/Systemkürzel sind gesperrt. Während ungültiger Eingaben gelten weiterhin die Defaults. Ctrl bezeichnet auf macOS die Command-Taste. Settings und Kernnavigation sind teilweise Deutsch/Englisch übersetzt; vollständige Übersetzung sämtlicher Fachdialoge bleibt ausdrücklich offen.

Abnahme: Drei Tests prüfen Persistenz und fehlerhafte Präferenzen, Shortcutkonflikte und beschädigte Fontdaten. Gesamtstand: 97 Tests grün, Clippy mit Warnungen als Fehler erfolgreich. Native Sichtprüfung prüfte History, alte/neue Bildvorschau, Suche, Settings und Light/Dark; Kontrastfehler dunkler Auswahl-/Diffhintergründe im hellen Theme wurden korrigiert.

### AppImage-Regel

Seit der Benutzeranweisung vom 3. November wird nach jeweils vier weiteren Commits ein neues, nach Commit benanntes AppImage gebaut. Prüfung: frisches AppDir, Vergleich der extrahierten ELF-Build-ID mit dem Releasebinary und direkter Start des tatsächlichen AppImages. Nach der Paketprüfung geht die Featurearbeit weiter. Die Regel ist keine Erledigt-Markierung für noch offene Features.

### Neue Fenster und Single Instance

Luna-Paket `window_features`: „Open in new window“ startet einen eigenständigen Repositoryprozess. In einem AppImage wird die AppImage-Datei gestartet, statt ein Executable aus einem später entfernten Mount wiederzuverwenden. `--new-window` umgeht die Unix-Single-Instance-Weiterleitung ausdrücklich; die CLI verarbeitet Repositorypfade nach diesem Schalter korrekt und behält den Rebase-Sequence-Editor-Sondermodus bei.

Normale weitere Starts übergeben Öffnen/Fokus an die vorhandene Unix-Instanz. Ein privates, eigentümergebundenes 0700-Verzeichnis, Dateilock, restriktiver Socket und begrenztes Binärprotokoll schützen Bindung/Weiterleitung/Stale-Cleanup. Während einer Git-Operation warten Öffnungsanfragen in der Queue. Unverfügbarkeit wird sichtbar gemeldet; nicht-Unix-Systeme laufen ohne Weiterleitungsgarantie weiter.

Abnahme: Drei Tests prüfen Argumente/Pfadgrenzen, AppImage-vs-Entwicklungs-Executable und echte lokale IPC einschließlich fehlerhafter Nachrichten und Cleanup. Der Socket-Test wurde wegen Sandbox-EPERM außerhalb der Socket-Sandbox ausgeführt. Gesamtstand nach Hosting-/Window-Integration: 105 Tests grün; Clippy erfolgreich.

### Hosting- und Browser-Code-Review-Workflows (Teilfeature)

Luna-Paket `hosting_features`: Integrations erkennt HTTPS-/SSH-/SCP-Remotes für GitHub, GitLab, Gitea, Bitbucket Cloud und Beanstalk. Explizite Aktionen öffnen Repository, getrackten Branch, ausgewählten Commit und eine vorausgefüllte Vergleichs-/PR-/MR- bzw. Review-Seite. Branchlinks verwenden das tatsächliche Upstream-Remote, Compare respektiert Ziel-/Quellrepository. Custom Hosts unterstützen geprüfte relative Routen. Konfiguration wird pro Repository gespeichert; Credentialanteile, nicht-HTTP(S)-Webadressen und Query-/Fragmentinhalte in eigenen Repository-URLs sind ausgeschlossen.

Dies nutzt die Browseranmeldung und ist ausdrücklich keine native Kontenverwaltung, PR-API-Liste oder API-Erstellung. Diese Teile bleiben offen. Fünf Tests prüfen providerbezogene Links, Sonderzeichen/Forks, SSH-Ports, ungültige Schemes und Credentialredaktion; keine privaten Konten oder Netzwerkaktionen wurden benutzt.

Verifizierte Primärquellen für die Browserworkflows: [GitHub PR Query-Parameter](https://docs.github.com/en/pull-requests/reference/using-query-parameters-to-create-a-pull-request), [GitLab Merge-Workflow](https://docs.gitlab.com/topics/git/merge/), [Bitbucket PR-Links](https://support.atlassian.com/bitbucket-cloud/kb/terminal-not-showing-create-pull-request-for-branch-http-link-to-bitbucket-repo/), [Beanstalk Branch-Reviews](https://support.beanstalkapp.com/article/65-how-do-i-request-a-code-review-for-a-branch), [Gitea PR-API-Dokumentation](https://docs.gitea.com/api/operations/repo-create-pull-request/) als Referenz zum separat offenen API-Ausbau.

### Korrektur: Unstage gelöschter Zeilen bei Kontext 0

Die Abnahme des selektiven Discard deckte einen gemeinsamen Patchfehler auf: Eine umgekehrte reine Löschung muss die Einfügeposition für `--unidiff-zero` berücksichtigen. Die Korrektur liegt nun zentral in `diff_features::selected_patch`; ein bisher nur auf Discard angewandter Workaround wurde entfernt. Eine echte Git-Regression stagt drei entfernte Löschungen, unstagt selektiv zwei davon und prüft exakt wiederhergestellte Indexzeilen, verbleibende staged Löschung und unveränderten Arbeitsbaum. Die bisherigen Patch-/Discard-Tests und Clippy bleiben grün.
