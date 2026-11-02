# GittyUp → grit: Funktionsanalyse und Implementierungsplan

Stand: 2. November 2026. Ziel: native Rust-Implementierung mit egui und libgit2; vertraute GittyUp-Arbeitsabläufe. Dies ist eine überprüfbare Bestandsaufnahme, keine Behauptung vollständiger Funktionsgleichheit.

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
| Repository-Chooser und neue Fenster | Teilweise | Menü mit Recent-Liste vorhanden; Mehrfenster offen |
| Diff- und History-Bereich maximieren | Fehlt | `Ctrl/Cmd+M`, temporärer Fokusmodus |
| Baum-/Listen-/Doppelbaum-Dateiansicht | Staging-Doppelliste vorhanden | Echte Verzeichnisbäume bleiben Ausbau |
| Log mit Operationen und Fehlern | Nur aktuelle Statusmeldung | Begrenzte Sitzungshistorie mit vollständiger Ausgabe ergänzen |
| Dateisystemänderungen automatisch erkennen | Fehlt | Gedrosseltes Refresh bei Fokuswechsel; voller Watcher offen |
| Terminal und Dateimanager öffnen | Fehlt | Plattformabhängige explizite Benutzeraktionen; keine Shell-Interpolation |
| Sprache, Fonts, Theme, Hotkeys konfigurierbar | Dunkles Theme, wenige Shortcuts | Umfangreiche Einstellungsoberfläche offen |
| Single-instance, Auto-Update, Plugins | Fehlt | Eigene Infrastruktur, nach Kernparität |

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
| Diff-Kontext, Ergänzungen, Löschungen, Hunk-Scope durchsuchen | Fehlt | Separater Ausbau; nicht als vorhandene Volltextsuche ausgeben |
| Kommentare, String-Literale, Identifier durchsuchen | Fehlt | GittyUps Lexer/Index besitzt dafür eigene Infrastruktur; Rust-Indexer offen |
| Suchvorschläge, Query-Sprache mit Gruppierung | Fehlt | Nicht mit einfachem Substring-Suchfeld gleichsetzen |
| Commit komplett lesen, Autor und Committer unterscheiden | Nur Summary und Autor | Vollständige Message/E-Mail/Committer/Eltern ergänzen |
| Dateiversionen/Dateiverlauf und vorherige/nächste Revision | Nur aktueller Text/Blame | Luna-Dateiversionspaket |
| Blame mit integrierter Timeline | Nur `git blame`-Text | Dateiverlauf ergänzt; interaktive Blame-Zeitleiste offen |

### Änderungen, Diff und Staging

Quelle: `DiffView/HunkWidget.cpp`, `DiffView/Images.cpp`, `FileContextMenu.cpp`, `DoubleTreeWidget.cpp`, `DiffPanel.cpp`.

| Funktion | grit am Anfang | Umsetzung / Abnahme |
|---|---|---|
| Getrennte vorgemerkte/nicht vorgemerkte Änderungen | Vorhanden | Gleiche Datei auf beiden Seiten korrekt getrennt |
| Datei/alle Dateien stagen und unstagen | Vorhanden | Tatsächliche Indexinhalte testen |
| Einzelne Hunks stagen | Vorhanden | Gegen falschen oder veralteten Patch absichern |
| Einzelne Zeilen auswählen und stagen/unstagen | Nur manuelles Patch-Editing | Luna-Patchpaket; echte Apply-Tests für Vorwärts-/Rückwärtsrichtung |
| Tastatur S/U/R und Markierung im Diff | Fehlt | S/U für selektierte Änderungen; Discard weiter mit Bestätigung |
| Dateien/ausgewählte Änderungen verwerfen | Dateien vorhanden | Zeilenweises Discard bleibt getrennte, bestätigte Funktion |
| Ungetrackte Dateien ausblenden | Fehlt | Anzeigepräferenz; Daten nicht verändern |
| Ignore für einzelne Dateien und Muster | Fehlt | Luna-Repositorypaket; escaping, bestehende `.gitignore`, Symlinks testen |
| Alte/neue Zeilennummern und farbige Herkunft | Vorhanden | Diff-Origin-Regression vorhanden |
| Dynamischer Zeilenumbruch | Fehlt | Renderoption, Patch-Rohdaten unverändert |
| Kontextanzahl, Whitespace ignorieren, Encoding auswählen | Fehlt | Kontext/Whitespace als reine Anzeigeoptionen; Nicht-UTF8 offen |
| Bild-/Binärvorschau, gelöschte Bilder | Fehlt | Binärdateien sichtbar erkennen; Bildrenderer bleibt Ausbau |
| Große Diffs erst nach Nachfrage laden | Fehlt | Größen-/Zeilenlimit und explizites »Load« ergänzen |
| Dateiversion exportieren/mit Anwendung öffnen | Fehlt | Luna-Dateiversionspaket, bytegetreuer Export, Überschreibbestätigung |
| Dateipfad kopieren, Dateiverlauf filtern, Executable-Bit ändern | Fehlt | Kontextmenüs schrittweise erweitern |
| Externer Editor/Diff/Merge | Fehlt | Konfigurierbare argv-Werkzeuge mit lokalem Prozessstart; kein Shelltext |

### Commits, Branches und Konflikte

Quelle: `CommitEditor.cpp`, `TemplateDialog.cpp`, `AmendDialog.cpp`, `MergeDialog.cpp`, `TagDialog.cpp`, `RepoView.cpp`.

| Funktion | grit am Anfang | Umsetzung / Abnahme |
|---|---|---|
| Normale Commits und Amend | Vorhanden | Bestehenden Workflow erhalten |
| Wiederverwendbare Commit-Vorlage und Git `commit.template` | Fehlt | Luna-Commitpaket; nicht ungefragt vorhandene Eingabe überschreiben |
| Mehrere benannte Vorlagen, Sortieren/Import/Export | Fehlt | Weiterer Ausbau nach einfacher Vorlage |
| Commit-Autor überschreiben, Sign-off | Fehlt | Luna-Commitpaket; argv, Identität und echte Commit-Metadaten testen |
| Autor/Committer-Zeitstempel gezielt ändern | Fehlt | Eigener Amend-Dialog als späterer Ausbau |
| Branch erstellen, sicher löschen, Checkout | Vorhanden | Keine erzwungenen Löschungen |
| Branch umbenennen | Fehlt | Luna-Repositorypaket |
| Branch mergen; Konflikte fortführen/abbrechen | Rebase vorhanden, Merge-Start fehlt | Luna-Repositorypaket plus Merge-Abschluss in Changes |
| Rebase, interaktives Rebase, Squash/Fixup/Drop | Vorhanden als Planeditor | Weitere grafische Planbedienung offen |
| Cherry-pick/Revert und deren Konfliktfortsetzung | Start vorhanden | Fortsetzen/Abbrechen zustandsabhängig ergänzen |
| Detached Checkout / Branch an Commit | Vorhanden | Weiterverwenden |
| Soft/Mixed/Hard Reset | Fehlt | Eigene explizite Ziel-/Auswirkungsbestätigung nötig; kein Force-Shortcut |
| Leichte und annotierte Tags, Löschen | Nur leichtes Tag | Annotieren/Löschen im Luna-Repositorypaket |
| Tag aus bestehendem Namen ableiten; Tag pushen/ersetzen | Fehlt | Tagliste als Vorlage; Force bleibt gesonderter Ausbau |
| Stash speichern, anwenden, poppen, löschen | Vorhanden | Eigene Namen/Optionen ergänzen |
| Konflikte Dateiweise ours/theirs/manuell lösen | Vorhanden | Binärspezifische Vorschau fehlt; kein Auto-Abbruch nach Konflikt |

### Remotes, Hosting und größere Repositories

Quelle: `RemoteDialog.cpp`, `ConfigDialog.cpp`, `UpdateSubmodulesDialog.cpp`, `RemoteCallbacks.cpp`, `host/*`, `cred/*`.

| Funktion | grit am Anfang | Umsetzung / Abnahme |
|---|---|---|
| Remotes verwalten, fetch, pull, push, upstream veröffentlichen | Vorhanden | Pull derzeit ausdrücklich nur fast-forward |
| Bestimmten Remote/Branch wählen, Tracking konfigurieren | Teilweise | Detailverwaltung weiter ausbauen |
| Automatisches Fetch und Prune-Option | Prune bei manuellem Fetch | Automatisches Netzwerkverhalten als Opt-in, kein heimlicher Default |
| Push force-with-lease / Tags | Fehlt | Gesonderte Ziel- und Auswirkungenkontrolle |
| Git-Credential-Helper und SSH-Agent | System-Git vorhanden | Kein eigener unsicherer Passwortspeicher |
| Hosting-Konten: GitHub/GitLab/Gitea/Bitbucket/Beanstalk | Fehlt | GittyUp besitzt konkrete Adapter; API-Featureparität offen |
| Pull-Request-Dialog und Hosting-Aktionen | Fehlt | Authentifizierung, Accountbindung und Browser/API-Integration erforderlich |
| Submodule ansehen, initialisieren, aktualisieren, öffnen | Fehlt | Luna-Repositorypaket; rekursive Updates explizit |
| Git LFS initialisieren, Tracking, Locks, Settings | Fehlt | Eigenes Paket; `git-lfs` muss vorhanden sein; nicht als allgemeine Git-Funktion vortäuschen |
| Worktrees und Bare-Repositories | libgit2 zum Teil vorhanden | Eigene Abnahmefälle; keine vollständige Verwaltungs-UI |
| SHA-256-Repositories | Nicht verifiziert | Nicht aus GittyUps Changelog auf git2-Rust-Unterstützung schließen |
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

Textzeilen-Staging unterstützt keine Binär-, Rename/Copy- oder reinen Mode-Patches; hierfür bleibt das Staging der gesamten Datei. Leere Dateien haben keine Textzeilen zum Selektieren. Nicht darstellbare partielle EOF-Patches werden von Git mit sichtbarer Fehlermeldung abgelehnt. Es gibt noch keinen lexikalischen Suchindex, keine Hosting-Konten-/PR-Verwaltung, keinen interaktiven Blame-Heatmap-Renderer, keine eingebettete Bildvorschau, keine vollständige externe Diff/Merge-Tool-Konfiguration und keine umfassenden Reset-/Force-Push-Dialoge. Diese Punkte bleiben Arbeitsvorrat aus der Matrix und werden nicht als erledigt ausgegeben.

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

Weiteres Paket dieser Runde: hierarchische Dateiansicht.
