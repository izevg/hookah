#!/bin/sh
# Resolves everything the scenarios need while online, so every scenario run is offline and fast.
# hookah itself never downloads anything: it runs Maven with `-o`.
set -e
mvn -B -q test-compile
mvn -B -q spotless:check || true
mvn -B -q checkstyle:checkstyle
mvn -B -q test -Dtest=CartTest -DexcludedGroups=integration
mvn -B -q org.pitest:pitest-maven:mutationCoverage -DtargetClasses=shop.Cart -DtargetTests=shop.CartTest -DoutputFormats=XML -DtimestampedReports=false
