#!/bin/sh
# Installs the locked dependencies; needs the npm registry on the first run. The story scenarios
# also need Playwright's Chromium (`npx playwright install chromium`), see their `requires`.
npm ci --no-audit --no-fund --loglevel=error
