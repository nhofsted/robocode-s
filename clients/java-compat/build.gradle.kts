// SPDX-License-Identifier: EPL-2.0

// The robocode-s robot host for classic Java robots (design §8). It runs classic
// Robocode's own robot side (robocode.api/core/host/repository 1.11.1: proxies, event
// manager, blocking calls, custom events, data files) and replaces only the peer that
// talks to the engine with one that speaks the robocode-s protocol.
//
// Classic finds its modules by jar name (robocode.*.jar), so the distribution keeps the
// classic jars as separate files: ./gradlew :clients:java-compat:installDist
plugins {
    application
    alias(libs.plugins.protobuf)
}

repositories {
    mavenCentral()
}

dependencies {
    implementation(libs.robocode.classic.api)
    implementation(libs.robocode.classic.core)
    implementation(libs.robocode.classic.host)
    implementation(libs.robocode.classic.repository)
    // Classic's container setup expects the battle module (RobocodeMain needs IRecordManager).
    runtimeOnly(libs.robocode.classic.battle)
    // Classic ships the Kotlin runtime for robots written in Kotlin.
    runtimeOnly(libs.kotlin.stdlib.classic)
    implementation(libs.protobuf.java)
    implementation(project(":clients:java-determinism"))

    testImplementation(platform(libs.junit.bom))
    testImplementation(libs.junit.jupiter)
    testRuntimeOnly(libs.junit.platform.launcher)
}

protobuf {
    protoc {
        artifact = "com.google.protobuf:protoc:${libs.versions.protobuf.get()}"
    }
}

sourceSets {
    main {
        proto {
            srcDir(layout.settingsDirectory.dir("proto"))
        }
    }
}

application {
    mainClass = "io.github.nhofsted.robocodes.compat.RobotHost"
    applicationName = "robot-host"
}

tasks.withType<JavaCompile>().configureEach {
    options.release = 21
}

tasks.test {
    useJUnitPlatform()
}
