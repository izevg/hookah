#!/bin/sh
# Resolves everything the scenarios need while online, so every scenario run is offline and fast.
# hookah itself never downloads anything: it runs Maven with `-o`.
set -e
mvn -B -q test-compile
mvn -B -q test-compile checkstyle:checkstyle -pl app -am
mvn -B -q test -Dtest=MoneyTest,ReceiptTest -Dsurefire.failIfNoSpecifiedTests=false -DexcludedGroups=integration
