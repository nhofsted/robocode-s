/*
 * SPDX-License-Identifier: EPL-2.0
 */
package io.github.nhofsted.robocodes.compat;

import io.github.nhofsted.robocodes.protocol.v1.ArcShape;
import io.github.nhofsted.robocodes.protocol.v1.DrawString;
import io.github.nhofsted.robocodes.protocol.v1.Line;
import io.github.nhofsted.robocodes.protocol.v1.PaintOp;
import io.github.nhofsted.robocodes.protocol.v1.Polygon;
import io.github.nhofsted.robocodes.protocol.v1.Rect;
import net.sf.robocode.robotpaint.Graphics2DSerialized;

import java.awt.BasicStroke;
import java.awt.Color;
import java.awt.Composite;
import java.awt.Font;
import java.awt.FontMetrics;
import java.awt.Graphics;
import java.awt.Graphics2D;
import java.awt.GraphicsConfiguration;
import java.awt.Image;
import java.awt.Paint;
import java.awt.Rectangle;
import java.awt.RenderingHints;
import java.awt.Shape;
import java.awt.Stroke;
import java.awt.font.FontRenderContext;
import java.awt.font.GlyphVector;
import java.awt.geom.AffineTransform;
import java.awt.geom.Arc2D;
import java.awt.geom.Ellipse2D;
import java.awt.geom.Line2D;
import java.awt.geom.PathIterator;
import java.awt.geom.Point2D;
import java.awt.geom.Rectangle2D;
import java.awt.geom.RoundRectangle2D;
import java.awt.image.BufferedImage;
import java.awt.image.BufferedImageOp;
import java.awt.image.ImageObserver;
import java.awt.image.RenderedImage;
import java.awt.image.renderable.RenderableImage;
import java.text.AttributedCharacterIterator;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Turns a robot's debug painting into robocode-s {@link PaintOp}s. Classic's robot side
 * records {@code onPaint} into a {@link Graphics2DSerialized}; {@link #ops} replays those
 * calls onto this recorder. Shapes keep their kind (line, rectangle, oval, arc) while no
 * transform is set and become polygons otherwise. Images, clipping and composites are
 * not painted. Coordinates are the robot's: battlefield units, y up.
 */
final class PaintRecorder extends Graphics2D {

    /** For font metrics and text layout, which robots may use to place strings. */
    private static final Graphics2D SCRATCH = new BufferedImage(1, 1, BufferedImage.TYPE_INT_ARGB).createGraphics();

    /** Where ops go, and the style the receiver already has (shared by {@link #create} copies). */
    private static final class Sink {
        final List<PaintOp> ops;
        Integer color;
        float stroke = Float.NaN;
        float fontSize = Float.NaN;

        Sink(List<PaintOp> ops) {
            this.ops = ops;
        }
    }

    private final Sink sink;
    private AffineTransform transform = new AffineTransform();
    private Color color = Color.WHITE;
    private Color background = Color.BLACK;
    private Stroke stroke = new BasicStroke(1);
    private Font font = new Font(Font.DIALOG, Font.PLAIN, 12);
    private Composite composite = SCRATCH.getComposite();
    private Paint paint = color;
    private final RenderingHints hints = new RenderingHints(null);

    private PaintRecorder(Sink sink) {
        this.sink = sink;
    }

    /**
     * The paint operations of a robot's queued graphics calls: none without calls. A
     * failure only loses the rest of this turn's painting, never the robot's turn.
     */
    static List<PaintOp> ops(Object calls) {
        List<PaintOp> ops = new ArrayList<>();
        // The first byte is the byte order; without calls after it there is nothing to do.
        if (calls instanceof byte[] bytes && bytes.length > 1) {
            try {
                Graphics2DSerialized decoder = new Graphics2DSerialized();
                decoder.setPaintingEnabled(true);
                decoder.processTo(new PaintRecorder(new Sink(ops)), bytes);
            } catch (RuntimeException e) {
                // Keep what was recorded.
            }
        }
        return ops;
    }

    private void style() {
        int argb = color.getRGB();
        if (sink.color == null || argb != sink.color) {
            sink.ops.add(PaintOp.newBuilder().setSetColor(argb).build());
            sink.color = argb;
        }
        float width = stroke instanceof BasicStroke b ? b.getLineWidth() : 1f;
        if (width != sink.stroke) {
            sink.ops.add(PaintOp.newBuilder().setSetStrokeWidth(width).build());
            sink.stroke = width;
        }
    }

    private void shape(Shape s, boolean fill) {
        style();
        List<io.github.nhofsted.robocodes.protocol.v1.Shape> shapes = convert(s);
        for (var shape : shapes) {
            sink.ops.add(fill ? PaintOp.newBuilder().setFill(shape).build() : PaintOp.newBuilder().setDraw(shape).build());
        }
    }

    private List<io.github.nhofsted.robocodes.protocol.v1.Shape> convert(Shape s) {
        var b = io.github.nhofsted.robocodes.protocol.v1.Shape.newBuilder();
        if (transform.isIdentity()) {
            if (s instanceof Line2D l) {
                return List.of(b.setLine(Line.newBuilder().setX1(l.getX1()).setY1(l.getY1())
                        .setX2(l.getX2()).setY2(l.getY2())).build());
            }
            if (s instanceof Rectangle2D r) {
                return List.of(b.setRect(rect(r)).build());
            }
            if (s instanceof Ellipse2D e) {
                return List.of(b.setOval(rect(e.getFrame())).build());
            }
            if (s instanceof Arc2D a && a.getArcType() == Arc2D.OPEN) {
                return List.of(b.setArc(ArcShape.newBuilder().setFrame(rect(a.getFrame()))
                        .setStart(a.getAngleStart()).setExtent(a.getAngleExtent())).build());
            }
        }
        // Anything else, or any shape under a transform: its outline, flattened.
        List<io.github.nhofsted.robocodes.protocol.v1.Shape> shapes = new ArrayList<>();
        PathIterator it = s.getPathIterator(transform, 0.25);
        double[] c = new double[6];
        Polygon.Builder poly = null;
        while (!it.isDone()) {
            switch (it.currentSegment(c)) {
                case PathIterator.SEG_MOVETO -> {
                    flush(shapes, poly, true);
                    poly = Polygon.newBuilder().addXs(c[0]).addYs(c[1]);
                }
                case PathIterator.SEG_LINETO -> {
                    if (poly == null) {
                        poly = Polygon.newBuilder();
                    }
                    poly.addXs(c[0]).addYs(c[1]);
                }
                case PathIterator.SEG_CLOSE -> {
                    flush(shapes, poly, false);
                    poly = null;
                }
                default -> {
                }
            }
            it.next();
        }
        flush(shapes, poly, true);
        return shapes;
    }

    private static void flush(List<io.github.nhofsted.robocodes.protocol.v1.Shape> shapes, Polygon.Builder poly,
            boolean open) {
        if (poly != null && poly.getXsCount() > 1) {
            shapes.add(io.github.nhofsted.robocodes.protocol.v1.Shape.newBuilder()
                    .setPolygon(poly.setOpen(open)).build());
        }
    }

    private static Rect rect(Rectangle2D r) {
        return Rect.newBuilder().setX(r.getX()).setY(r.getY()).setWidth(r.getWidth()).setHeight(r.getHeight()).build();
    }

    // Graphics2D: what robots paint with.

    @Override
    public void draw(Shape s) {
        shape(s, false);
    }

    @Override
    public void fill(Shape s) {
        shape(s, true);
    }

    @Override
    public void drawString(String str, float x, float y) {
        style();
        float size = font.getSize2D();
        if (size != sink.fontSize) {
            sink.ops.add(PaintOp.newBuilder().setSetFontSize(size).build());
            sink.fontSize = size;
        }
        Point2D p = transform.transform(new Point2D.Double(x, y), null);
        sink.ops.add(PaintOp.newBuilder().setDrawString(DrawString.newBuilder().setText(str)
                .setX(p.getX()).setY(p.getY())).build());
    }

    @Override
    public void drawString(String str, int x, int y) {
        drawString(str, (float) x, (float) y);
    }

    @Override
    public void drawString(AttributedCharacterIterator iterator, float x, float y) {
        StringBuilder text = new StringBuilder();
        for (char c = iterator.first(); c != AttributedCharacterIterator.DONE; c = iterator.next()) {
            text.append(c);
        }
        drawString(text.toString(), x, y);
    }

    @Override
    public void drawString(AttributedCharacterIterator iterator, int x, int y) {
        drawString(iterator, (float) x, (float) y);
    }

    @Override
    public void drawChars(char[] data, int offset, int length, int x, int y) {
        drawString(new String(data, offset, length), x, y);
    }

    @Override
    public void drawGlyphVector(GlyphVector g, float x, float y) {
        fill(g.getOutline(x, y));
    }

    @Override
    public void drawLine(int x1, int y1, int x2, int y2) {
        draw(new Line2D.Double(x1, y1, x2, y2));
    }

    @Override
    public void drawRect(int x, int y, int width, int height) {
        draw(new Rectangle2D.Double(x, y, width, height));
    }

    @Override
    public void fillRect(int x, int y, int width, int height) {
        fill(new Rectangle2D.Double(x, y, width, height));
    }

    @Override
    public void clearRect(int x, int y, int width, int height) {
        Color c = color;
        color = background;
        fillRect(x, y, width, height);
        color = c;
    }

    @Override
    public void drawRoundRect(int x, int y, int width, int height, int arcWidth, int arcHeight) {
        draw(new RoundRectangle2D.Double(x, y, width, height, arcWidth, arcHeight));
    }

    @Override
    public void fillRoundRect(int x, int y, int width, int height, int arcWidth, int arcHeight) {
        fill(new RoundRectangle2D.Double(x, y, width, height, arcWidth, arcHeight));
    }

    @Override
    public void drawOval(int x, int y, int width, int height) {
        draw(new Ellipse2D.Double(x, y, width, height));
    }

    @Override
    public void fillOval(int x, int y, int width, int height) {
        fill(new Ellipse2D.Double(x, y, width, height));
    }

    @Override
    public void drawArc(int x, int y, int width, int height, int startAngle, int arcAngle) {
        draw(new Arc2D.Double(x, y, width, height, startAngle, arcAngle, Arc2D.OPEN));
    }

    @Override
    public void fillArc(int x, int y, int width, int height, int startAngle, int arcAngle) {
        fill(new Arc2D.Double(x, y, width, height, startAngle, arcAngle, Arc2D.PIE));
    }

    @Override
    public void drawPolyline(int[] xPoints, int[] yPoints, int nPoints) {
        style();
        Polygon.Builder poly = Polygon.newBuilder().setOpen(true);
        for (int i = 0; i < nPoints; i++) {
            Point2D p = transform.transform(new Point2D.Double(xPoints[i], yPoints[i]), null);
            poly.addXs(p.getX()).addYs(p.getY());
        }
        sink.ops.add(PaintOp.newBuilder().setDraw(
                io.github.nhofsted.robocodes.protocol.v1.Shape.newBuilder().setPolygon(poly)).build());
    }

    @Override
    public void drawPolygon(int[] xPoints, int[] yPoints, int nPoints) {
        draw(new java.awt.Polygon(xPoints, yPoints, nPoints));
    }

    @Override
    public void fillPolygon(int[] xPoints, int[] yPoints, int nPoints) {
        fill(new java.awt.Polygon(xPoints, yPoints, nPoints));
    }

    // State.

    @Override
    public Color getColor() {
        return color;
    }

    @Override
    public void setColor(Color c) {
        if (c != null) {
            color = c;
            paint = c;
        }
    }

    @Override
    public void setPaint(Paint p) {
        paint = p;
        if (p instanceof Color c) {
            color = c;
        }
    }

    @Override
    public Paint getPaint() {
        return paint;
    }

    @Override
    public void setStroke(Stroke s) {
        stroke = s;
    }

    @Override
    public Stroke getStroke() {
        return stroke;
    }

    @Override
    public Font getFont() {
        return font;
    }

    @Override
    public void setFont(Font f) {
        if (f != null) {
            font = f;
        }
    }

    @Override
    public FontMetrics getFontMetrics(Font f) {
        return SCRATCH.getFontMetrics(f);
    }

    @Override
    public FontRenderContext getFontRenderContext() {
        return SCRATCH.getFontRenderContext();
    }

    @Override
    public void setBackground(Color c) {
        background = c;
    }

    @Override
    public Color getBackground() {
        return background;
    }

    @Override
    public void setComposite(Composite c) {
        composite = c;
    }

    @Override
    public Composite getComposite() {
        return composite;
    }

    @Override
    public void translate(int x, int y) {
        transform.translate(x, y);
    }

    @Override
    public void translate(double tx, double ty) {
        transform.translate(tx, ty);
    }

    @Override
    public void rotate(double theta) {
        transform.rotate(theta);
    }

    @Override
    public void rotate(double theta, double x, double y) {
        transform.rotate(theta, x, y);
    }

    @Override
    public void scale(double sx, double sy) {
        transform.scale(sx, sy);
    }

    @Override
    public void shear(double shx, double shy) {
        transform.shear(shx, shy);
    }

    @Override
    public void transform(AffineTransform tx) {
        transform.concatenate(tx);
    }

    @Override
    public void setTransform(AffineTransform tx) {
        transform = new AffineTransform(tx);
    }

    @Override
    public AffineTransform getTransform() {
        return new AffineTransform(transform);
    }

    @Override
    public void setRenderingHint(RenderingHints.Key key, Object value) {
        hints.put(key, value);
    }

    @Override
    public Object getRenderingHint(RenderingHints.Key key) {
        return hints.get(key);
    }

    @Override
    public void setRenderingHints(Map<?, ?> map) {
        hints.clear();
        hints.putAll(map);
    }

    @Override
    public void addRenderingHints(Map<?, ?> map) {
        hints.putAll(map);
    }

    @Override
    public RenderingHints getRenderingHints() {
        return (RenderingHints) hints.clone();
    }

    @Override
    public GraphicsConfiguration getDeviceConfiguration() {
        return SCRATCH.getDeviceConfiguration();
    }

    // Not painted: clipping, images, XOR mode.

    @Override
    public void clip(Shape s) {
    }

    @Override
    public void clipRect(int x, int y, int width, int height) {
    }

    @Override
    public void setClip(int x, int y, int width, int height) {
    }

    @Override
    public void setClip(Shape clip) {
    }

    @Override
    public Shape getClip() {
        return null;
    }

    @Override
    public Rectangle getClipBounds() {
        return null;
    }

    @Override
    public boolean hit(Rectangle rect, Shape s, boolean onStroke) {
        return false;
    }

    @Override
    public void setPaintMode() {
    }

    @Override
    public void setXORMode(Color c) {
    }

    @Override
    public void copyArea(int x, int y, int width, int height, int dx, int dy) {
    }

    @Override
    public boolean drawImage(Image img, AffineTransform xform, ImageObserver obs) {
        return true;
    }

    @Override
    public void drawImage(BufferedImage img, BufferedImageOp op, int x, int y) {
    }

    @Override
    public void drawRenderedImage(RenderedImage img, AffineTransform xform) {
    }

    @Override
    public void drawRenderableImage(RenderableImage img, AffineTransform xform) {
    }

    @Override
    public boolean drawImage(Image img, int x, int y, ImageObserver observer) {
        return true;
    }

    @Override
    public boolean drawImage(Image img, int x, int y, int width, int height, ImageObserver observer) {
        return true;
    }

    @Override
    public boolean drawImage(Image img, int x, int y, Color bgcolor, ImageObserver observer) {
        return true;
    }

    @Override
    public boolean drawImage(Image img, int x, int y, int width, int height, Color bgcolor, ImageObserver observer) {
        return true;
    }

    @Override
    public boolean drawImage(Image img, int dx1, int dy1, int dx2, int dy2, int sx1, int sy1, int sx2, int sy2,
            ImageObserver observer) {
        return true;
    }

    @Override
    public boolean drawImage(Image img, int dx1, int dy1, int dx2, int dy2, int sx1, int sy1, int sx2, int sy2,
            Color bgcolor, ImageObserver observer) {
        return true;
    }

    @Override
    public Graphics create() {
        PaintRecorder copy = new PaintRecorder(sink);
        copy.transform = new AffineTransform(transform);
        copy.color = color;
        copy.paint = paint;
        copy.stroke = stroke;
        copy.font = font;
        copy.background = background;
        return copy;
    }

    @Override
    public void dispose() {
    }
}
