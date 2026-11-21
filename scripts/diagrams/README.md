# Docs diagrams

The diagrams of the docs are drawn from sources in the repository, so anyone
can change one.

- Sources: `docs/images/3.0/diagrams/<name>.json`, in the JSON of
  [archify](https://github.com/tt-a1i/archify): `architecture` for the C4
  diagrams, `workflow` for the flows.
- Pictures: `<name>.svg` next to each source, drawn by
  `scripts/render-diagrams.mjs`. The pages show the SVG.
- Tools: archify 3.2.0, pinned to one commit of its repository, and
  Playwright 1.64.0. Both are pinned in `package.json` and
  `package-lock.json` in this folder.

## Change a diagram

1. Install the tools once, in this folder:

   ```sh
   npm ci
   npx playwright install chromium
   ```

2. Edit the source, for example `docs/images/3.0/diagrams/flow-forecast.json`.
3. Draw it from the top of the repository:
   `node scripts/render-diagrams.mjs flow-forecast`. With no name, it draws
   every diagram.
4. Commit the source and its SVG together.

`node scripts/render-diagrams.mjs --check` draws every diagram into a temp dir
and fails when a committed SVG is not what its source draws, when a source has
no SVG, or when a picture has no source. The `Diagrams` job of
`.github/workflows/site-preview.yml` runs it on every pull request that
touches the docs.

## Add a diagram

Copy a source of the same kind, change it, and draw it. A node can name the
code it shows in `sources`. archify checks each path and line range at the
commit that `meta.repository.revision` pins, so pin a commit of `main`, and
draw in a clone that has that commit (a shallow clone does not). The archify
reference is in its repository, under `archify/references`.

## Why SVG

- It is 70 % smaller than the PNG of the same diagram.
- It stays sharp at any zoom.
- It carries its own fonts, so it is the same on every machine. The PNG
  export draws the title in a font of the system, so its bytes differ from
  one machine to the next, and CI could not check it.
- Its text can be found and copied when the picture is opened on its own.

## Update archify

Change the commit in `package.json`, run `npm install` in this folder, draw
every diagram, and commit the new SVGs with the new lockfile.
