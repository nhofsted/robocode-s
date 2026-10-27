// SPDX-License-Identifier: EPL-2.0

// Robots made for our own tests (not shipped): ones that do what real robots do and the
// samples don't, such as starting threads. Compiled against classic robocode.api.
plugins {
    java
}

repositories {
    mavenCentral()
}

dependencies {
    compileOnly(libs.robocode.classic.api)
}
