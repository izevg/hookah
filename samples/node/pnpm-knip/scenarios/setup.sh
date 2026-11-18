#!/bin/sh
# Installs the locked dependencies; needs the npm registry on the first run.
pnpm install --frozen-lockfile --reporter=silent
