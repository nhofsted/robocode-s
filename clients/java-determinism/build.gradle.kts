// SPDX-License-Identifier: EPL-2.0

// Java agent that gives every robot its own seeded random number streams
// (design §7.2). Used by the difftest oracle and by the robocode-s Java runner.
plugins {
    `java-library`
}

repositories {
    mavenCentral()
}

dependencies {
    implementation(libs.asm)

    testImplementation(platform(libs.junit.bom))
    testImplementation(libs.junit.jupiter)
    testRuntimeOnly(libs.junit.platform.launcher)
}

tasks.withType<JavaCompile>().configureEach {
    options.release = 21
}

// The agent jar is self-contained: -javaagent puts only this jar on the class path.
tasks.jar {
    manifest {
        attributes(
            "Premain-Class" to "io.github.nhofsted.robocodes.determinism.DeterminismAgent",
            "Can-Retransform-Classes" to "true",
        )
    }
    from(configurations.runtimeClasspath.map { classpath -> classpath.map { zipTree(it) } }) {
        exclude("module-info.class", "META-INF/**")
    }
    duplicatesStrategy = DuplicatesStrategy.EXCLUDE
}

tasks.test {
    useJUnitPlatform()
    dependsOn(tasks.jar)
    val agentJar = tasks.jar.flatMap { it.archiveFile }
    jvmArgumentProviders.add(CommandLineArgumentProvider { listOf("-javaagent:${agentJar.get().asFile}") })
}
