// Prints classic Robocode's recolouring of the robot sprites as JSON, for sprites.spec.ts.
// From web/: javac -d /tmp/rc -cp robocode.api-1.11.1.jar <classic src>/robocode.ui/src/main/java/net/sf/robocode/ui/gfx/ColorUtil.java tools/Recolor.java
//   java -cp /tmp/rc:robocode.api-1.11.1.jar net.sf.robocode.ui.gfx.Recolor src/lib/assets/classic/{body,turret,radar}.png > src/lib/components/recolor.expected.json
package net.sf.robocode.ui.gfx;
import java.awt.Color;
import java.awt.image.BufferedImage;
import java.io.File;
import java.util.TreeSet;
import javax.imageio.ImageIO;

// Prints classic's recolouring (ImageUtil.ColorFilter.filterRGB on top of the real ColorUtil)
// for every distinct pixel colour of the given images and a few robot colours, as JSON.
public class Recolor {
	public static void main(String[] args) throws Exception {
		TreeSet<Integer> pixels = new TreeSet<>();
		for (String path : args) {
			BufferedImage img = ImageIO.read(new File(path));
			for (int y = 0; y < img.getHeight(); y++)
				for (int x = 0; x < img.getWidth(); x++) {
					int argb = img.getRGB(x, y);
					if ((argb >>> 24) != 0) pixels.add(argb & 0xffffff);
				}
		}
		int[] colors = { 0xff0000, 0x3f8a4f, 0x1d6ea3, 0xffc25c, 0x808080, 0xffffff, 0x000000, 0x6a2fb0 };
		StringBuilder out = new StringBuilder("[\n");
		boolean first = true;
		for (int c : colors) {
			Color color = new Color(c);
			float[] hsl = ColorUtil.fromRGBtoHSL(color.getRed(), color.getGreen(), color.getBlue());
			for (int p : pixels) {
				int r = (p >> 16) & 0xff, g = (p >> 8) & 0xff, b = p & 0xff;
				float[] HSL = ColorUtil.fromRGBtoHSL(r, g, b);
				int result = p;
				if (HSL[1] > 0) {
					float L = Math.min(1, (hsl[2] + HSL[2]) / 2 + hsl[2] / 7);
					result = ColorUtil.fromHSLtoRGB(hsl[0], hsl[1], L) & 0xffffff;
				}
				out.append(first ? "" : ",\n").append("[").append(c).append(",").append(p).append(",").append(result).append("]");
				first = false;
			}
		}
		System.out.println(out.append("\n]"));
	}
}
