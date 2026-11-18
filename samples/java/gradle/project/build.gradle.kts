plugins {
    java
    checkstyle
    id("com.diffplug.spotless") version "7.0.4"
    id("info.solidsoft.pitest") version "1.19.0"
}

group = "shop"
version = "1.0.0"

java {
    toolchain {
        languageVersion = JavaLanguageVersion.of(21)
    }
}

repositories {
    mavenCentral()
}

dependencies {
    testImplementation(platform("org.junit:junit-bom:5.12.2"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.test {
    useJUnitPlatform()
}

checkstyle {
    toolVersion = "10.26.1"
}

spotless {
    java {
        googleJavaFormat("1.28.0")
    }
}

pitest {
    junit5PluginVersion = "1.2.2"
    pitestVersion = "1.20.0"
    outputFormats = setOf("XML")
    timestampedReports = false
    targetClasses = setOf("shop.*")
    enableDefaultIncrementalAnalysis = true
}
