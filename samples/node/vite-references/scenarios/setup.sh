#!/bin/sh
# Installs the locked dependencies; needs the npm registry on the first run.
npm ci --no-audit --no-fund --loglevel=error
