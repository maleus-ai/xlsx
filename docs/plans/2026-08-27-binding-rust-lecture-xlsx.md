# Plan — Binding Rust pour la lecture XLSX en streaming

Date : 2026-08-27
Portée : nouveau dépôt public `maleus-ai/xlsx` (Rust + binding Node), consommé ensuite par `.claude/skills/patterns-import-export/`.
Destinataire : agent dédié, contexte neuf. Ce document se lit sans rien connaître de l'enquête qui l'a produit.

## Pourquoi

Les applications générées par Maleus reçoivent des imports de données par upload. La recette canonique (`.claude/skills/patterns-import-export/resources/imports.md`) impose de ne jamais charger un fichier uploadé en mémoire : l'upload va sur disque via `diskStorage`, puis il est streamé vers une staging table via `COPY`. Cette forme tient pour le CSV. Elle ne tient pour aucun format compressé, parce qu'aucune librairie JavaScript de lecture XLSX ne réunit les trois propriétés dont on a besoin.

Ces trois propriétés sont non négociables, et chacune vient d'un incident ou d'une mesure :

1. **Lecture en streaming à mémoire bornée.** Une app cliente est morte en production sur un import. Sur 200 000 lignes × 20 colonnes, une lecture bufferisée réclame 2,1 à 2,5 Go de pic RSS ; les VM de production ont 4 Go.
2. **Typage correct des dates.** Dans un XLSX, une date est un nombre plus un `numFmt` déclaré dans `xl/styles.xml`. Sans cette table, `45376` est indiscernable d'une quantité. Une librairie qui ne l'expose pas écrit des nombres là où le métier attend des dates — une valeur fausse en base, pas une exception.
3. **Multi-feuilles.** Un classeur métier a des onglets : données, paramètres, légende, notes.

Trois candidats JavaScript ont été mesurés (protocole en `## Annexes`) :

| Lib                        | multi-feuilles              | 600k lignes                     | dates | dernière publi |
| -------------------------- | --------------------------- | ------------------------------- | ----- | -------------- |
| `exceljs` 4.4.0            | **mort à 5 onglets** (0/20) | 292 Mo                          | ✅    | 2024-12        |
| `xlsx-stream-reader` 1.1.1 | ✅                          | 112 Mo (mémoire plate)          | ❌    | 2022-06        |
| `read-excel-file` 9.3.10   | ✅                          | ❌ `RangeError` (pile dépassée) | ✅    | 2026-08        |

Aucune ne coche les trois cases. `exceljs`, le seul à typer les dates _et_ streamer, perd la queue de son archive dès que plusieurs feuilles sont mises en attente sur disque : la marche d'entrées s'arrête après la dernière feuille, `xl/sharedStrings.xml`, `xl/styles.xml` et `xl/workbook.xml` ne sont jamais lus, et le lecteur throw sur `Cannot read properties of undefined (reading 'sheets')` en abandonnant ~180 descripteurs pour 30 lectures. Mesuré sur `exceljs` nu, sans code appelant, en lecture complète : 19/20 succès à 4 onglets, **0/20 à 5**.

Un fork corrigeant ce défaut a été prototypé et fonctionne (12 lignes). Il reste un fork d'une librairie de 19 673 lignes dont l'amont ne publie plus, à porter indéfiniment.

`calamine`, la librairie Rust de référence pour ce format, réunit les trois propriétés nativement. C'est ce que ce plan vient exploiter.

## Ce qui change

**Avant**

- La lecture XLSX repose sur `exceljs`, qui échoue silencieusement ou bruyamment au-delà de 4 onglets et n'expose aucune borne mémoire.
- Les bornes de sécurité sont posées en JavaScript, en amont du parseur, dans une passe de décompression supplémentaire qui coûte +15 % de temps et qu'un appelant peut oublier.
- Chaque défaut découvert se corrige par une couche de contournement au-dessus de la librairie, jamais dans la marche d'archive elle-même.

**Après**

- Un dépôt public `maleus-ai/xlsx` publie `@maleus/xlsx-reader`, un lecteur XLSX en streaming écrit en Rust sur `calamine`, avec un binding Node.
- Les bornes — octets décompressés, nombre de lignes — sont appliquées **dans le lecteur**, où la marche d'archive lui appartient. Elles ne peuvent pas être oubliées par l'appelant et ne coûtent pas de passe supplémentaire.
- Le lecteur rend des lignes typées par lots. La sémantique produit (gabarit d'en-tête, refus multi-feuilles, forme des records) reste côté JavaScript, dans la recette, où elle est relisible et testée.

## Architecture

```
                       fichier uploadé sur disque
                     (diskStorage, chemin, pas un flux)
                                   │
                                   ▼
        ┌──────────────────────────────────────────────────┐
        │  @maleus/xlsx-reader  (paquet npm, binaire natif) │
        │                                                   │
        │   JS      XlsxRows extends Readable (objectMode)  │
        │    │        pull → nextBatch()                    │
        │    ▼                                              │
        │   napi-rs  AsyncTask sur le threadpool libuv      │
        │    │        (ne bloque jamais l'event loop)       │
        │    ▼                                              │
        │   Rust     calamine::Xlsx                         │
        │             · worksheet_cells_reader(name)        │
        │             · CellFormat → dates typées           │
        │             · compteur d'octets décompressés      │
        │             · compteur de lignes                  │
        └───────────────────────┬──────────────────────────┘
                                │ lots de lignes typées
                                ▼
        ┌──────────────────────────────────────────────────┐
        │  parseXlsx (recette Maleus, JavaScript)           │
        │   · en-tête → clés des records                    │
        │   · refus multi-feuilles, validation d'en-tête    │
        │   · mapping vers BackendError                     │
        └───────────────────────┬──────────────────────────┘
                                ▼
                     streamingImport → COPY → staging
```

Trois invariants que le schéma ne montre pas.

**Le lecteur prend un chemin, pas un flux.** Un ZIP se lit par son central directory, en fin d'archive : une lecture correcte exige `Seek`. `exceljs` s'en passait en parcourant séquentiellement, et c'est précisément cette approximation qui perd la queue de l'archive. Le lecteur assume donc un fichier sur disque — ce que `diskStorage` produit déjà.

**Le dépôt public ne connaît pas Maleus.** Aucune politique d'import, aucun `BackendError`, aucune notion de recette. Il expose un lecteur et des bornes ; tout ce qui est décision produit reste dans la recette. C'est la condition pour que le dépôt soit publiable et testable seul.

**Le binding ne bloque jamais l'event loop.** Le parsing tourne sur le threadpool, pas sur le thread principal, et les lignes traversent la frontière FFI par lots. Un appel FFI par ligne sur 600 000 lignes coûterait plus cher que le parsing lui-même.

## Décisions

### 1. `calamine` plutôt qu'un lecteur Rust maison

`calamine` 0.36.1 (mise à jour 2026-07-27, 3,5 M de téléchargements récents) gère le contrat implicite du format : `sharedStrings`, `numFmt`, `inlineStr`, système de dates 1904, valeurs de formules en cache. Chaque trou dans ce contrat n'est pas une exception mais une valeur fausse écrite en base.

Point vérifié, et c'est lui qui rend le binding mince : **`Xlsx::worksheet_cells_reader(name)` est public** et rend un `XlsxCellReader` qui streame cellule par cellule, en câblant en interne la table des shared strings et les `CellFormat`. Aucun fork de `calamine` n'est nécessaire, et le typage des dates est acquis.

```rust
let mut workbook: Xlsx<_> = open_workbook(path)?;
let mut reader = workbook.worksheet_cells_reader("Data")?;
while let Some(cell) = reader.next_cell()? { … }
```

### 2. `napi-rs` plutôt que `neon`

Ce qu'on achète dans un framework de binding ici n'est pas l'ergonomie de l'API Rust — le binding fait quelques centaines de lignes — mais le **pipeline de release multi-cible**.

|                | version         | dernière maj | téléchargements récents |
| -------------- | --------------- | ------------ | ----------------------- |
| `napi`         | 3.12.2          | 2026-08-21   | 13,7 M                  |
| `neon`         | 1.1.1           | 2025-12-05   | 1,07 M                  |
| `@napi-rs/cli` | 3.8.6           | 2026-08-12   | —                       |
| `@neon-rs/cli` | 0.2.6 (pré-1.0) | 2026-03-10   | —                       |

La cible de déploiement des apps générées est `node:24-alpine`, donc **`x86_64-unknown-linux-musl`**. `napi-rs` traite musl comme une cible de première classe ; le README de `neon` ne mentionne ni musl ni la cross-compilation, et l'outillage dont on dépendrait le plus y est en 0.2.

Argument de fond : tout ce chantier existe parce qu'une dépendance a cessé d'être maintenue sans qu'on le voie. Choisir le moins actif des deux frameworks reproduirait l'erreur qu'on répare.

### 3. Un dépôt public, MIT, séparé du monorepo — nom neutre, paquets étroits

`maleus-ai/xlsx`, public, licence MIT, branche par défaut `master` — les conventions de `maleus-ai/kepler`, déjà en Rust dans l'organisation.

Le dépôt est nommé pour le **format**, pas pour l'opération, et les paquets pour l'opération :

```
maleus-ai/xlsx
├── crates/xlsx-core          lecture (calamine), aucune dépendance napi
├── crates/xlsx-node          binding napi        → @maleus/xlsx-reader
└── crates/xlsx-writer-core   éventuellement plus tard → @maleus/xlsx-writer
```

Deux raisons, et aucune n'est cosmétique. Un nom de dépôt se change mal (remotes, docs, champ `repository` du paquet) et un nom de paquet publié ne se change pas du tout : la neutralité coûte zéro aujourd'hui et évite une migration plus tard. À l'inverse, **un binaire natif ne se tree-shake pas** — lecture et écriture compilées dans le même `cdylib` feraient embarquer les deux à tout consommateur. Un dépôt, une matrice CI, des paquets minces.

Public et séparé n'est pas un geste d'affichage : c'est ce qui **force l'API à rester générique**. Un lecteur qui ne peut pas importer `BackendError` ne peut pas encoder de politique Maleus par accident. Et il devient testable, benchmarkable et publiable indépendamment du monorepo.

### 4. Les bornes vivent dans le lecteur, pas au-dessus

C'est la raison d'être du chantier autant que la correction du format. Une borne posée au-dessus d'une librairie ne protège de rien pendant les phases que la librairie exécute avant de rendre sa première ligne — et pour un XLSX ces phases construisent quatre tables, toutes dimensionnées par le fichier, aucune par le nombre de lignes. Mesuré sur `exceljs`, sur des classeurs portant **une seule ligne de données** : 324 Ko d'upload atteignent 423 Mo de RSS via `sharedStrings`, 1,04 Mo atteignent 843 Mo via `styles`, 1,57 Mo atteignent 2,1 Go via les relations, 0,50 Mo atteignent 2,5 Go via le modèle du classeur.

Le lecteur porte donc :

- `max_decompressed_bytes` — compté sur les octets qui sortent réellement de l'inflater, jamais sur les tailles déclarées par l'archive (une archive fabriquée ment dessus) ;
- `max_rows` — compté sur les lignes rendues.

Les deux sont **obligatoires dans l'API** : pas de valeur par défaut permissive, pas d'opt-in. Un appelant qui ne veut pas de borne doit l'écrire explicitement.

### 5. Le lecteur rend des lignes, la recette rend des records

Découpage de responsabilité, et il faut s'y tenir pour que la décision 3 tienne :

|                                                 | lecteur Rust | recette JavaScript |
| ----------------------------------------------- | ------------ | ------------------ |
| ouverture de l'archive, bornes                  | ✅           |                    |
| lister les feuilles, en choisir une par nom     | ✅           |                    |
| valeurs typées, dates en ISO 8601 UTC           | ✅           |                    |
| lignes creuses rendues à leur indice de colonne | ✅           |                    |
| en-tête → clés des records                      |              | ✅                 |
| refus multi-feuilles, validation d'en-tête      |              | ✅                 |
| mapping vers `BackendError` et code HTTP        |              | ✅                 |

`XlsxCellReader::next_cell()` rend des **cellules**, pas des lignes : l'assemblage des lignes et le placement des cellules creuses à leur indice sont à faire côté Rust, sur la base de la référence `r="B7"` de chaque cellule.

### 6. Un protocole par lots, tiré par le consommateur

Le binding expose un curseur, pas un callback : le JavaScript appelle `nextBatch()`, le Rust rend jusqu'à `batch_size` lignes. Le modèle « tiré » (pull) plutôt que « poussé » (push) évite les `ThreadsafeFunction` et fait tomber la contre-pression toute seule — un `Readable` en `objectMode` n'appelle `_read()` que lorsque son buffer se vide.

Taille de lot par défaut : 1 000 lignes. À valider par mesure (voir `## Vérification`), l'ordre de grandeur visant à rendre le coût FFI négligeable devant le parsing sans retenir plus de quelques Mo.

### 7. Ce que `@maleus/xlsx-reader` ne fera pas

Pas d'écriture XLSX, pas de CSV, pas d'ODS, pas de formules recalculées, pas d'API synchrone. Le périmètre du **paquet** est la lecture en streaming d'un `.xlsx` sur disque, bornée. Tout ajout ultérieur doit repasser par cette liste.

Le garde-fou est ici, pas dans le nom du dépôt (décision 3) : un dépôt neutre ne doit pas devenir une invitation à l'élargissement, et c'est cette liste qui tient le périmètre.

**Sur l'écriture, la porte reste ouverte et le chemin est connu.** Elle demanderait un second crate — `calamine` est un lecteur, l'écriture c'est `rust_xlsxwriter` (0.99.0, mis à jour 2026-08-23, 1,74 M de téléchargements récents), donc un `crates/xlsx-writer-core` et un paquet `@maleus/xlsx-writer` distincts, partageant seulement la matrice de release.

Mais rien n'y presse, et il faut le dire pour que personne ne s'y engage par symétrie : **aucune des raisons de ce chantier ne s'applique à l'écriture.** L'impossibilité du Duplex vient de l'ordre des parties dans une archive qu'on subit ; en écriture on choisit cet ordre. Les bornes viennent d'une entrée non fiable ; en écriture la donnée est la nôtre. Le writer streaming d'`exceljs` fonctionne. Ajouter un writer serait une décision d'opportunité, pas une correction.

## Implémentation

### Phase 1 — Le socle Rust

1. Créer `maleus-ai/xlsx` : public, MIT, `master` par défaut, workspace Cargo sur le modèle de `kepler` (`resolver = "2"`, `[workspace.package] version`, `[profile.release] strip = true`).

2. Crate `xlsx-core`, sans aucune dépendance napi : c'est ce qui garantit que le cœur reste testable en Rust pur.

   ```rust
   pub struct ReaderOptions {
       pub max_decompressed_bytes: u64,
       pub max_rows: u64,
   }

   pub enum CellValue {
       Empty,
       Text(String),
       Number(f64),
       Bool(bool),
       /// ISO 8601 UTC, jamais une locale.
       DateTime(String),
       Error(String),
   }

   pub struct SheetInfo { pub name: String, pub visible: bool }

   impl XlsxReader {
       pub fn open(path: &Path, options: ReaderOptions) -> Result<Self, ReadError>;
       pub fn sheets(&self) -> &[SheetInfo];
       pub fn select(&mut self, name: &str) -> Result<(), ReadError>;
       /// Rend au plus `max` lignes ; `Ok(None)` marque la fin de la feuille.
       pub fn next_batch(&mut self, max: usize) -> Result<Option<Vec<Vec<CellValue>>>, ReadError>;
   }
   ```

   → vérifier : `xlsx-core` compile sans `napi` dans son arbre de dépendances (`cargo tree -p xlsx-core | grep -c napi` rend 0).

3. Implémenter les bornes dans la marche d'archive. `max_decompressed_bytes` se compte sur les octets sortis de l'inflater, entrée par entrée, en cumulé sur l'archive entière — jamais sur les tailles annoncées par le central directory.

   → vérifier : une archive déclarant 64 octets pour une entrée qui en déploie 64 Mo est refusée.

4. Assembler les lignes à partir des cellules. Une cellule porte sa référence (`r="B7"`) : une ligne se ferme au changement d'indice de ligne, et chaque cellule se place à son indice de colonne, les trous restant `CellValue::Empty`.

   → vérifier : un classeur dont une ligne a une cellule vide au milieu et une autre des cellules vides en fin rend, pour les deux, un vecteur de la largeur de la feuille.

5. Erreurs typées, exhaustives, sans `unwrap` dans le chemin de lecture.

   ```rust
   pub enum ReadError {
       NotAnArchive,
       SheetNotFound { name: String, available: Vec<String> },
       DecompressedBudgetExceeded { limit: u64, entry: String },
       RowBudgetExceeded { limit: u64 },
       Corrupt { detail: String },
   }
   ```

   → vérifier : `grep -rn "unwrap()\|expect(" src/` ne remonte rien hors tests.

### Phase 2 — Le binding Node

6. Crate `xlsx-node` : `napi-rs`, `crate-type = ["cdylib"]`. Chaque méthode qui lit rend une `AsyncTask` — le parsing tourne sur le threadpool libuv, jamais sur le thread principal.

   → vérifier : une lecture de 600 000 lignes n'empêche pas un `setInterval(…, 10)` de tirer à sa cadence pendant toute la durée.

7. Façade JavaScript, la seule surface publique du paquet :

   ```ts
   export type XlsxRowsOptions = {
     /** Feuille ciblée. Par défaut, la première. */
     sheet?: string;
     /** Obligatoire. Octets décompressés autorisés, toutes entrées confondues. */
     maxDecompressedBytes: number;
     /** Obligatoire. Lignes de données autorisées. */
     maxRows: number;
     /** Lignes par aller-retour FFI. Défaut 1000. */
     batchSize?: number;
   };

   /** Liste les feuilles sans lire une seule ligne. */
   export function listSheets(
     path: string,
   ): Promise<Array<{ name: string; visible: boolean }>>;

   /** Readable objectMode : une ligne = un tableau de valeurs, à l'indice de colonne. */
   export function xlsxRows(path: string, options: XlsxRowsOptions): Readable;
   ```

   Exemple d'appel, tel que la recette l'utilisera :

   ```ts
   const [sheet] = await listSheets(file.path);

   for await (const row of xlsxRows(file.path, {
     sheet: sheet.name,
     maxDecompressedBytes: MAX_XLSX_BYTES,
     maxRows: MAX_IMPORT_ROWS,
   })) {
     // row: Array<string | number | boolean | null>
   }
   ```

   → vérifier : `listSheets` sur un classeur de 600 000 lignes rend en moins de 200 ms — elle ne doit lire que `xl/workbook.xml`.

8. Types TypeScript générés et publiés avec le paquet ; aucun `any` dans la surface publique.

### Phase 3 — Release multi-cible

9. Copier la matrice de `maleus-ai/kepler` (`.github/workflows/release.yml`), qui couvre déjà les cibles nécessaires : `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `aarch64-apple-darwin`. Elle installe la toolchain musl-cross depuis `musl.cc` et tourne sur runners blacksmith.

   **`x86_64-unknown-linux-musl` est la cible de production** : les apps générées tournent sur `node:24-alpine`, plateforme épinglée dans leur Dockerfile.

   → vérifier : `docker run --rm -v …:/app node:24-alpine node -e "require('@maleus/xlsx-reader')"` charge le binaire dans l'image de déploiement réelle.

10. Publication npm : un paquet racine `@maleus/xlsx-reader` et un paquet par triple en `optionalDependencies`, selon la convention `napi-rs`. Un futur writer serait un paquet séparé (décision 7), pas une extension de celui-ci.

    → vérifier : une installation sur une cible non publiée échoue à l'installation avec un message explicite, et non au premier appel en production.

11. CI de test : `cargo test` sur Linux, plus la suite Node sur au moins une cible native, sur le modèle des `tests.yml` / `test-macos.yml` de `kepler`.

### Phase 4 — Le retour dans la recette Maleus

Cette phase se traite dans le monorepo, **après** la publication du paquet, et fait l'objet de sa propre PR.

12. Réécrire `.claude/skills/patterns-import-export/resources/parseXlsx.ts` sur `xlsxRows` : la sémantique produit décrite en décision 5 est déjà écrite et testée, seul le moteur change. Le contrat public (`Duplex` de records keyés par l'en-tête) doit être préservé — c'est le critère qui prouve que la couche d'interface était bien indépendante du moteur.

13. Supprimer `assertXlsxWithinBudget.ts` et sa dépendance `unzipper` : les bornes vivent désormais dans le lecteur (décision 4), et la pré-passe qui coûtait +15 % n'a plus de raison d'être.

14. Reprendre le plan `docs/plans/2026-08-26-streaming-xlsx-et-skill-backend-external-data.md` : sa décision 2 (« `exceljs` plutôt qu'une brique maison ») est caduque et doit renvoyer ici.

## Vérification

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
pnpm test                      # suite Node du dépôt
```

Critères d'acceptation, tous chiffrés sur les mesures qui ont motivé le chantier (fixtures en `## Annexes`) :

- [ ] Un classeur à 2, 4, 8 et 16 onglets se lit intégralement, 20 fois de suite, sans échec et sans croissance du nombre de descripteurs ouverts.
- [ ] 600 000 lignes × 10 colonnes se lisent sous **150 Mo** de pic RSS — la référence étant les 112 Mo du seul lecteur JS réellement streaming, et les 292 Mo d'`exceljs`.
- [ ] Le pic RSS ne suit pas le nombre de lignes : entre 200 000 et 600 000 lignes, il ne doit pas tripler.
- [ ] Une colonne de dates ressort en ISO 8601 UTC, indépendamment de la locale et du fuseau du process, y compris sur un classeur en système de dates 1904.
- [ ] Les cinq archives hostiles (`sharedStrings`, `styles`, `rels`, `workbook`, `inlineStr`) sont refusées avec `DecompressedBudgetExceeded`, à pic mémoire plat, sans que la taille de l'archive influe sur le pic.
- [ ] Une archive qui **ment sur ses tailles déclarées** est refusée sur l'expansion réelle.
- [ ] `max_rows` coupe sur la ligne qui le dépasse, pas à la fin de la feuille.
- [ ] Une lecture longue ne retarde pas un timer de plus de 50 ms — preuve que l'event loop n'est pas bloqué.
- [ ] Le binaire `x86_64-unknown-linux-musl` se charge dans `node:24-alpine`.

Risques :

| Risque                                                            | Sévérité | Mitigation                                                                                                                                                                                                                                          |
| ----------------------------------------------------------------- | -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `calamine` ne streame pas aussi bien que son API le laisse croire | Élevée   | **À lever en premier** : un prototype de 30 lignes lisant 600 000 lignes en mesurant le RSS, avant toute autre ligne de code. Si le pic suit le nombre de lignes, le plan s'arrête ici et on revient au fork `exceljs`, qui est prototypé et mesuré |
| Le coût FFI par lot domine le parsing                             | Moyenne  | Mesurer la taille de lot sur 100 / 1 000 / 10 000 avant de figer le défaut                                                                                                                                                                          |
| Une cible manquante casse un `pnpm install` client                | Moyenne  | La matrice `kepler` couvre déjà les cibles utiles ; échec à l'installation avec message explicite plutôt qu'au runtime                                                                                                                              |
| Le dépôt public absorbe de la logique Maleus                      | Moyenne  | Décision 3 et le contrôle « pas de `napi` ni de politique produit dans le core » ; toute exception doit être argumentée dans une PR du dépôt                                                                                                        |
| Deux moteurs à maintenir pendant la transition                    | Faible   | Le fork `exceljs` n'est pas livré ; la PR #3260 du monorepo reste en draft jusqu'à la phase 4                                                                                                                                                       |

## Hors scope

- **L'écriture de fichiers XLSX.** Les exports restent sur la pile actuelle.
- **Le CSV et le JSONL.** Leur chemin d'import ne change pas ; il n'a jamais eu ce problème.
- **La recette d'intégration Maleus.** Phase 4 seulement, dans le monorepo, après publication.
- **Le fork `exceljs`.** Prototypé et mesuré (12 lignes, 20/20 sur 2/4/8/16 onglets), il reste le plan B si le risque n°1 se matérialise. Il n'est ni livré ni publié.

## Annexes

### Fixtures à reconstruire

Aucune n'est fournie : elles se régénèrent, et le faire est le premier exercice utile.

| Fixture                      | Construction                                                                                              | Sert à                       |
| ---------------------------- | --------------------------------------------------------------------------------------------------------- | ---------------------------- |
| `sheets-{2,4,8,16}.xlsx`     | writer bufferisé d'`exceljs`, 30 lignes par feuille                                                       | régression multi-onglets     |
| `large-{200000,600000}.xlsx` | archive assemblée à la main : `sharedStrings` de 5 000 mots, colonne de dates en `numFmt 14`, 10 colonnes | mémoire et débit             |
| `bomb-sharedstrings.xlsx`    | 3 M d'entrées `<si>`, une seule ligne de données                                                          | borne, phase amont           |
| `bomb-styles.xlsx`           | 3 M de `<xf>`                                                                                             | borne, phase amont           |
| `bomb-rels.xlsx`             | 2 M de `<Relationship>`                                                                                   | borne, phase amont           |
| `bomb-workbook.xlsx`         | 2 M de `<sheet>`                                                                                          | borne, phase amont           |
| `bomb-inline.xlsx`           | une cellule `inlineStr` de 300 Mo                                                                         | borne, phase ligne           |
| `lying-sizes.xlsx`           | central directory déclarant 64 octets pour 64 Mo réels                                                    | borne sur l'expansion réelle |

Les archives hostiles sont **assemblées octet par octet** (en-tête local, deflate brut, central directory, EOCD), jamais écrites par une librairie : le point est de produire ce qu'une librairie refuserait d'écrire. `zlib.crc32` et `zlib.deflateRawSync` suffisent, aucune dépendance nécessaire.

### Mesures de référence

Toutes obtenues sur le même poste, un process par mesure, `/proc/self/fd` pour les descripteurs, échantillonnage RSS à 5 ms.

`exceljs` 4.4.0 non modifié, lecture complète, TMPDIR par défaut, 20 runs :

| Onglets | feuille nommée | lecture complète |
| ------- | -------------- | ---------------- |
| 2       | 20/20          | 20/20            |
| 3       | 19/20          | 20/20            |
| 4       | 16/20          | 19/20            |
| 5       | 1/20           | 0/20             |
| 8       | 0/20           | 0/20             |

Lecture d'un classeur de 200 000 et 600 000 lignes × 10 colonnes (zip 8,31 Mo et 24,94 Mo) :

| Lib                  | 200k           | 600k            |
| -------------------- | -------------- | --------------- |
| `exceljs`            | 249 Mo · 5,6 s | 292 Mo · 18,7 s |
| `xlsx-stream-reader` | 99 Mo · 19,6 s | 112 Mo · 55,9 s |
| `read-excel-file`    | 146 Mo · 4,9 s | `RangeError`    |

Classeurs hostiles portant **une seule ligne de données**, `exceljs` non borné :

| Vecteur         | Upload  | Pic RSS  | Issue              |
| --------------- | ------- | -------- | ------------------ |
| `sharedStrings` | 324 Ko  | 423 Mo   | aucun refus        |
| `styles`        | 1,04 Mo | 843 Mo   | aucun refus        |
| `rels`          | 1,57 Mo | 2 139 Mo | aucun refus        |
| `workbook`      | 0,50 Mo | 2 497 Mo | refus après le pic |

### Contexte à ne pas redécouvrir

- Le défaut d'`exceljs` a été localisé : `iterateStream(zip)` met en file les **objets entrée** pendant que l'archive continue de couler, et `tmp.file` ouvre un écart asynchrone entre l'émission d'une entrée et son `pipe`. Corriger les deux (marche native sur le stream `unzipper`, `tmp.fileSync` + `pipeline`) répare le multi-onglets. Utile si le plan B est activé.
- `unzipper` 0.12 ne change rien au défaut ; ce n'est pas une question de version de dépendance.
- La cible de déploiement des apps générées est `node:24-alpine`, plateforme épinglée `linux/amd64` dans leur Dockerfile, documentée dans `infra/templates/repository/docs/infra/deploiement-autonome.md`.
- Un module natif est déjà embarqué dans le template (`bcrypt` 6.0.0 via `node-gyp-build`) : le précédent existe, l'installation de natif dans cette image fonctionne.
