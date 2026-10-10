// SPDX-License-Identifier: EPL-2.0

subprojects {
    group = "io.github.nhofsted.robocodes"
    version = "0.1.0-SNAPSHOT"

    plugins.withType<JavaPlugin> {
        extensions.configure<JavaPluginExtension> {
            toolchain.languageVersion = JavaLanguageVersion.of(21)
        }
        tasks.withType<JavaCompile>().configureEach {
            options.encoding = "UTF-8"
            // Classic robocode.jar targets Java 8; robots built against our jar must too.
            options.release = 8
            options.compilerArgs.add("-Xlint:-options")
        }
    }
}
