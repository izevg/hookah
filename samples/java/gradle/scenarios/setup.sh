#!/bin/sh
# Resolves everything the scenarios need while online, so every scenario run is offline and fast.
# hookah itself never downloads anything: it runs Gradle with `--offline`.
set -e
gradle --console=plain -q compileTestJava checkstyleMain checkstyleTest spotlessJavaCheck test --tests=shop.CartTest
gradle --console=plain -q pitest --targetTests=shop.CartTest
