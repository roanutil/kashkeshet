# Provenance

Each start of the server is recorded as a server run:

- the version;
- the git commit it was built from (`-dirty` if the working tree had
  changes, `unknown` if built outside git);
- every setting in effect, including the config file read (for an access
  key, only the file's path);
- a SHA-256 of the instructions agents are given.

Every agent and message records the run that created it. The web page lists
the runs, and shows the run on each message and agent. Rows from before runs
were recorded have none.
