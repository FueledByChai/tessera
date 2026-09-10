Run the registered feature studies and append the findings to the research log. Follow AGENTS.md.

1. Study configs are `../Tessera-private/research/studies/*.toml`. Each carries its window in
   comment lines `# start = YYYY-MM-DD` and `# end = YYYY-MM-DD` (the lake has fixed coverage;
   do not invent dates). Skip a file that lacks them and say so in the report.
2. For each config, run from the public checkout:
   `./target/release/tessera study --config <file> --start <start> --end <end> --output-dir ../Tessera-private/research/results/<YYYY-MM-DD>/<config-name>`
   Build the engine first if the binary is missing or older than `src/`.
3. Read each result's `cells.csv` (symbol, feature, horizon, observations, IC, top-minus-bottom
   bps and t). Collect the ten cells with the largest |t| across all studies, and every cell with
   |t| above 3.
4. Append one dated entry to `../Tessera-private/docs/research-log.md`: which configs ran, their
   windows, the top-ten table, the flagged cells, and one or two sentences on what changed versus
   the previous entry for the same config (compare IC sign and magnitude). Do not editorialize
   beyond that; findings are for the owner to judge.
5. Commit the log entry in the private repo with the subject `research: nightly studies <date>`
   and the co-author trailer. Never push. Do not commit the results directory.
6. Report: configs run, the flagged cells, and anything that failed.
