// SPDX-License-Identifier: EPL-2.0

// Differential testing against classic Robocode (design §13). The oracle runs
// classic battles and records every turn; see README.md.
plugins {
    application
}

repositories {
    mavenCentral()
}

val sampleRobots = configurations.create("sampleRobots") {
    isCanBeConsumed = false
}
val determinismAgent = configurations.create("determinismAgent") {
    isCanBeConsumed = false
    isTransitive = false
}

dependencies {
    implementation(libs.robocode.classic.api)
    runtimeOnly(libs.robocode.classic.core)
    implementation(libs.robocode.classic.battle) // BoundingRectangle for GeometryVectors
    runtimeOnly(libs.robocode.classic.host)
    runtimeOnly(libs.robocode.classic.repository)
    implementation(project(":clients:java-determinism"))
    implementation(libs.asm)
    sampleRobots(project(":robots:samples"))
    determinismAgent(project(":clients:java-determinism"))

    testImplementation(platform(libs.junit.bom))
    testImplementation(libs.junit.jupiter)
    testRuntimeOnly(libs.junit.platform.launcher)
}

// JVM flags that make classic Robocode deterministic and give it StrictMath
// semantics (ADR 0003). Oracle checks at startup that they are in effect.
val oracleJvmArgs = listOf(
    "-Xmx512M",
    // Same module openings as classic's robocode.sh.
    "--add-opens=java.base/sun.net.www.protocol.jar=ALL-UNNAMED",
    "--add-opens=java.base/java.lang.reflect=ALL-UNNAMED",
    "-XX:+UnlockDiagnosticVMOptions",
    "-XX:-InlineMathNatives",
    "-Ddebug=true",
)

application {
    mainClass = "io.github.nhofsted.robocodes.difftest.Oracle"
    applicationDefaultJvmArgs = oracleJvmArgs
}

tasks.withType<JavaCompile>().configureEach {
    options.release = 21
}

class RobotJarsArgument(@get:InputFiles @get:PathSensitive(PathSensitivity.NONE) val jars: FileCollection) :
    CommandLineArgumentProvider {
    override fun asArguments() = listOf("-Drobocodes.difftest.robotJars=${jars.asPath}")
}

// Per-robot seeded randomness (design §7.2); the oracle refuses to run without it.
class AgentArgument(
    @get:InputFiles @get:PathSensitive(PathSensitivity.NONE) val agent: FileCollection,
    @get:Input val asProperty: Boolean,
) : CommandLineArgumentProvider {
    override fun asArguments() =
        if (asProperty) listOf("-Drobocodes.difftest.agentJar=${agent.singleFile}")
        else listOf("-javaagent:${agent.singleFile}")
}

class PathProperty(
    @get:Input val name: String,
    @get:InputFiles @get:PathSensitive(PathSensitivity.NONE) val files: FileCollection,
) : CommandLineArgumentProvider {
    override fun asArguments() = listOf("-D$name=${files.asPath}")
}

// The difftest jar doubles as an agent with timing fixes for classic (ClassicPatches).
tasks.jar {
    manifest {
        attributes("Premain-Class" to "io.github.nhofsted.robocodes.difftest.ClassicPatches")
    }
}
val patchesAgent: FileCollection = files(tasks.jar)

tasks.named<JavaExec>("run") {
    jvmArgumentProviders.add(RobotJarsArgument(sampleRobots))
    jvmArgumentProviders.add(AgentArgument(determinismAgent, false))
    jvmArgumentProviders.add(AgentArgument(patchesAgent, false))
}

tasks.test {
    useJUnitPlatform()
    jvmArgumentProviders.add(RobotJarsArgument(sampleRobots))
    jvmArgumentProviders.add(AgentArgument(determinismAgent, true))
    jvmArgumentProviders.add(PathProperty("robocodes.difftest.patchesJar", patchesAgent))
    systemProperty("robocodes.difftest.oracleJvmArgs", oracleJvmArgs.joinToString(" "))
}

// Test vectors for the Rust port of java.util.Random (crates/java-random).
tasks.register<JavaExec>("javaRandomVectors") {
    description = "Regenerates crates/java-random/tests/data/java_random.txt from the running JDK."
    classpath = sourceSets.main.get().runtimeClasspath
    mainClass = "io.github.nhofsted.robocodes.difftest.JavaRandomVectors"
    val output = layout.settingsDirectory.file("crates/java-random/tests/data/java_random.txt")
    outputs.file(output)
    args(output.asFile.path)
    doFirst { output.asFile.parentFile.mkdirs() }
}

// Test vectors for the Rust fdlibm port (crates/fdlibm).
tasks.register<JavaExec>("strictMathVectors") {
    description = "Regenerates crates/fdlibm/tests/data/strictmath.txt from the running JDK."
    classpath = sourceSets.main.get().runtimeClasspath
    mainClass = "io.github.nhofsted.robocodes.difftest.StrictMathVectors"
    val output = layout.settingsDirectory.file("crates/fdlibm/tests/data/strictmath.txt")
    outputs.file(output)
    args(output.asFile.path)
    doFirst { output.asFile.parentFile.mkdirs() }
}

// Test vectors for the Rust geometry crate (crates/geom).
tasks.register<JavaExec>("geometryVectors") {
    description = "Regenerates crates/geom/tests/data/geometry.txt from the running JDK."
    classpath = sourceSets.main.get().runtimeClasspath
    mainClass = "io.github.nhofsted.robocodes.difftest.GeometryVectors"
    jvmArgs("-XX:+UnlockDiagnosticVMOptions", "-XX:-InlineMathNatives")
    val output = layout.settingsDirectory.file("crates/geom/tests/data/geometry.txt")
    outputs.file(output)
    args(output.asFile.path)
    doFirst { output.asFile.parentFile.mkdirs() }
}
