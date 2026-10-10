// SPDX-License-Identifier: EPL-2.0

// Classic sample robots (EPL-2.0, see README.md). Compiled against classic
// robocode.api for now; later also against :clients:java-compat to check that
// our API is a drop-in replacement.
plugins {
    java
}

repositories {
    mavenCentral()
}

dependencies {
    compileOnly(libs.robocode.classic.api)
}
