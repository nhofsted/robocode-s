/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.compat;

import net.sf.robocode.core.Container;
import net.sf.robocode.core.ContainerBase;
import net.sf.robocode.host.IHostedThread;
import net.sf.robocode.host.IThreadManager;
import net.sf.robocode.host.io.RobotFileOutputStream;
import net.sf.robocode.host.io.RobotFileSystemManager;
import net.sf.robocode.host.security.ThreadManager;
import net.sf.robocode.security.HiddenAccess;
import net.sf.robocode.security.IThreadManagerBase;
import robocode.exception.RobotException;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;

/**
 * Classic's thread manager with 1.11.1's {@link #createRobotFileStream}, which opens a
 * robot's {@code RobocodeFileOutputStream}: 1.11.2 added a check there, outside its
 * security switch, that the file is directly in the robot's data folder, and punishes the
 * robot otherwise (ADR 0027). That changes battles: cf.RiO.RiOx 4.2.1 and hadur2.Hadur 3.12
 * write {@code getDataDirectory() + "\\" + name}, which is in the data folder on Windows
 * and beside it elsewhere, so 1.11.2 would disable them on Linux and macOS only. Until ADR
 * 0027 decides robots' file rules for every platform alike, robots write as with 1.11.1,
 * held to their home by the sandbox (ADR 0019) and to classic's quota.
 */
public class ClassicFileWrites extends ThreadManager {

    /**
     * Puts this in classic's container, before anything asks for the thread manager (one
     * instance serves the host manager and {@code RobocodeFileOutputStream} alike). Classic's
     * container finds its modules on the class path that {@code HiddenAccess.init} sets, which
     * {@code RobocodeEngine} calls again later (it does nothing the second time). The robot
     * host and the difftest oracle both call it, so that classic plays alike in both.
     */
    public static void install() {
        HiddenAccess.init();
        Container.cache.removeComponent(IThreadManager.class);
        Container.cache.addComponent(IThreadManager.class, ClassicFileWrites.class);
        if (!(ContainerBase.getComponent(IThreadManagerBase.class) instanceof ClassicFileWrites)) {
            throw new IllegalStateException("classic's thread manager was not replaced");
        }
    }

    /** Classic 1.11.1's, without its SecurityManager bookkeeping (robocode-s runs none). */
    @Override
    public FileOutputStream createRobotFileStream(String fileName, boolean append) throws IOException {
        IHostedThread robotProxy = getRobotProxy(Thread.currentThread());
        if (robotProxy == null) {
            System.err.println("RobotProxy is null");
            return null;
        }
        if (!robotProxy.getStatics().isAdvancedRobot()) {
            throw new RobotException("Only advanced robots could create files");
        }
        RobotFileSystemManager fileSystemManager = robotProxy.getRobotFileSystemManager();
        File dir = fileSystemManager.getWritableDirectory();
        if (!dir.exists()) {
            robotProxy.println("SYSTEM: Creating a data directory for you.");
            if (!dir.exists() && !dir.mkdirs()) {
                System.err.println("Can't create dir " + dir);
            }
        }
        File f = new File(fileName);
        long len;
        if (f.exists()) {
            len = f.length();
        } else {
            fileSystemManager.checkQuota();
            len = 0;
        }
        if (!append) {
            fileSystemManager.adjustQuota(-len);
        }
        return new RobotFileOutputStream(fileName, append, fileSystemManager);
    }
}
