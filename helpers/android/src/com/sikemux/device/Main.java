package com.sikemux.device;

import android.app.UiAutomation;
import android.graphics.Bitmap;
import android.graphics.Rect;
import android.os.Bundle;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.SystemClock;
import android.util.Base64;
import android.view.InputDevice;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.accessibility.AccessibilityNodeInfo;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.ByteArrayOutputStream;
import java.io.InputStreamReader;
import java.io.PrintStream;

/**
 * Drives the device for Sikemux from adb's shell, as scrcpy's server does: reads the screen
 * through the accessibility tree and injects touches and keys. One JSON request per line on
 * stdin, one answer per line on stdout, each carrying the request's id.
 */
public final class Main {
    private static final int MAX_DEPTH = 60;

    private final UiAutomation automation;
    private final PrintStream out;
    private long touchDown;

    private Main(UiAutomation automation, PrintStream out) {
        this.automation = automation;
        this.out = out;
    }

    public static void main(String[] args) throws Exception {
        Looper.prepareMainLooper();
        HandlerThread thread = new HandlerThread("sikemux-automation");
        thread.start();
        PrintStream out = new PrintStream(new java.io.FileOutputStream(java.io.FileDescriptor.out), true, "UTF-8");
        Main main = new Main(connect(thread.getLooper()), out);
        out.println("{\"type\":\"ready\"}");
        BufferedReader in = new BufferedReader(new InputStreamReader(System.in, "UTF-8"));
        String line;
        while ((line = in.readLine()) != null) {
            main.answer(line);
        }
        System.exit(0);
    }

    private static UiAutomation connect(Looper looper) throws Exception {
        Class<?> connection = Class.forName("android.app.UiAutomationConnection");
        Class<?> contract = Class.forName("android.app.IUiAutomationConnection");
        java.lang.reflect.Constructor<UiAutomation> made =
                UiAutomation.class.getDeclaredConstructor(Looper.class, contract);
        made.setAccessible(true);
        UiAutomation automation = made.newInstance(looper, connection.getDeclaredConstructor().newInstance());
        UiAutomation.class.getDeclaredMethod("connect", int.class)
                .invoke(automation, UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
        return automation;
    }

    private void answer(String line) {
        long id = 0;
        JSONObject reply;
        try {
            JSONObject request = new JSONObject(line);
            id = request.optLong("id");
            reply = handle(request);
            reply.put("type", "result");
        } catch (Throwable error) {
            reply = new JSONObject();
            try {
                reply.put("type", "error");
                reply.put("message", String.valueOf(error.getMessage() != null ? error.getMessage() : error));
            } catch (Exception ignored) {
            }
        }
        try {
            reply.put("id", id);
        } catch (Exception ignored) {
        }
        out.println(reply.toString());
    }

    private JSONObject handle(JSONObject request) throws Exception {
        switch (request.getString("type")) {
            case "tree":
                return tree();
            case "gesture":
                gesture(request.getJSONArray("frames"));
                return new JSONObject();
            case "touch":
                touch(request.getString("phase"), (float) request.getDouble("x"), (float) request.getDouble("y"));
                return new JSONObject();
            case "key":
                key(request.getInt("code"), request.optString("phase", "press"));
                return new JSONObject();
            case "text":
                return text(request.getString("text"));
            case "screenshot":
                return screenshot(request.optDouble("scale", 1), request.optString("format", "jpeg"));
            case "rotate":
                if (!automation.setRotation(request.getInt("rotation"))) {
                    throw new IllegalStateException("the device would not turn");
                }
                return new JSONObject();
            default:
                throw new IllegalArgumentException("unknown request " + request.getString("type"));
        }
    }

    private JSONObject tree() throws Exception {
        AccessibilityNodeInfo root = automation.getRootInActiveWindow();
        for (int tries = 0; root == null && tries < 30; tries++) {
            Thread.sleep(50);
            root = automation.getRootInActiveWindow();
        }
        JSONObject reply = new JSONObject();
        JSONArray nodes = new JSONArray();
        if (root != null) {
            reply.put("package", String.valueOf(root.getPackageName()));
            Rect bounds = new Rect();
            root.getBoundsInScreen(bounds);
            reply.put("bounds", rect(bounds));
            walk(root, nodes, 0);
        }
        reply.put("nodes", nodes);
        return reply;
    }

    private void walk(AccessibilityNodeInfo node, JSONArray nodes, int depth) throws Exception {
        if (node == null || depth > MAX_DEPTH) {
            return;
        }
        if (node.isVisibleToUser()) {
            JSONObject read = new JSONObject();
            read.put("class", String.valueOf(node.getClassName()));
            put(read, "text", node.getText());
            put(read, "description", node.getContentDescription());
            put(read, "id", node.getViewIdResourceName());
            put(read, "hint", node.getHintText());
            read.put("enabled", node.isEnabled());
            read.put("clickable", node.isClickable());
            read.put("editable", node.isEditable());
            read.put("password", node.isPassword());
            read.put("checkable", node.isCheckable());
            read.put("checked", node.isChecked());
            read.put("selected", node.isSelected());
            read.put("focused", node.isFocused());
            read.put("scrollable", node.isScrollable());
            Rect bounds = new Rect();
            node.getBoundsInScreen(bounds);
            read.put("bounds", rect(bounds));
            nodes.put(read);
        }
        for (int i = 0; i < node.getChildCount(); i++) {
            walk(node.getChild(i), nodes, depth + 1);
        }
    }

    private static void put(JSONObject read, String key, CharSequence value) throws Exception {
        if (value != null && value.length() > 0) {
            read.put(key, value.toString());
        }
    }

    private static JSONArray rect(Rect bounds) {
        return new JSONArray().put(bounds.left).put(bounds.top).put(bounds.right).put(bounds.bottom);
    }

    /** Fingers that go down on the first frame, follow each frame at its time, and lift after the last. */
    private void gesture(JSONArray frames) throws Exception {
        long down = SystemClock.uptimeMillis();
        int fingers = frames.getJSONObject(0).getJSONArray("pointers").length();
        float[][] at = points(frames.getJSONObject(0));
        inject(down, MotionEvent.ACTION_DOWN, at, 1);
        for (int finger = 1; finger < fingers; finger++) {
            inject(down, MotionEvent.ACTION_POINTER_DOWN | (finger << MotionEvent.ACTION_POINTER_INDEX_SHIFT), at, finger + 1);
        }
        for (int i = 1; i < frames.length(); i++) {
            JSONObject frame = frames.getJSONObject(i);
            long due = down + (long) (frame.getDouble("t") * 1000);
            long wait = due - SystemClock.uptimeMillis();
            if (wait > 0) {
                Thread.sleep(wait);
            }
            at = points(frame);
            inject(down, MotionEvent.ACTION_MOVE, at, fingers);
        }
        for (int finger = fingers - 1; finger > 0; finger--) {
            inject(down, MotionEvent.ACTION_POINTER_UP | (finger << MotionEvent.ACTION_POINTER_INDEX_SHIFT), at, finger + 1);
        }
        inject(down, MotionEvent.ACTION_UP, at, 1);
    }

    private static float[][] points(JSONObject frame) throws Exception {
        JSONArray pointers = frame.getJSONArray("pointers");
        float[][] at = new float[pointers.length()][2];
        for (int i = 0; i < pointers.length(); i++) {
            at[i][0] = (float) pointers.getJSONArray(i).getDouble(0);
            at[i][1] = (float) pointers.getJSONArray(i).getDouble(1);
        }
        return at;
    }

    /** One finger the person moves by hand, from a down to an up. */
    private void touch(String phase, float x, float y) {
        float[][] at = {{x, y}};
        switch (phase) {
            case "down":
                touchDown = SystemClock.uptimeMillis();
                inject(touchDown, MotionEvent.ACTION_DOWN, at, 1);
                break;
            case "move":
                inject(touchDown, MotionEvent.ACTION_MOVE, at, 1);
                break;
            default:
                inject(touchDown, MotionEvent.ACTION_UP, at, 1);
        }
    }

    private void inject(long down, int action, float[][] at, int fingers) {
        MotionEvent.PointerProperties[] properties = new MotionEvent.PointerProperties[fingers];
        MotionEvent.PointerCoords[] coords = new MotionEvent.PointerCoords[fingers];
        for (int i = 0; i < fingers; i++) {
            properties[i] = new MotionEvent.PointerProperties();
            properties[i].id = i;
            properties[i].toolType = MotionEvent.TOOL_TYPE_FINGER;
            coords[i] = new MotionEvent.PointerCoords();
            coords[i].x = at[i][0];
            coords[i].y = at[i][1];
            coords[i].pressure = 1;
            coords[i].size = 1;
        }
        MotionEvent event = MotionEvent.obtain(down, SystemClock.uptimeMillis(), action, fingers, properties, coords,
                0, 0, 1, 1, 0, 0, InputDevice.SOURCE_TOUCHSCREEN, 0);
        automation.injectInputEvent(event, true);
        event.recycle();
    }

    private void key(int code, String phase) {
        long now = SystemClock.uptimeMillis();
        if (!phase.equals("up")) {
            automation.injectInputEvent(new KeyEvent(now, now, KeyEvent.ACTION_DOWN, code, 0), true);
        }
        if (!phase.equals("down")) {
            automation.injectInputEvent(new KeyEvent(now, SystemClock.uptimeMillis(), KeyEvent.ACTION_UP, code, 0), true);
        }
    }

    /**
     * Types as a keyboard would. Characters no key makes, such as emoji, go in through the
     * focused field's text instead.
     */
    private JSONObject text(String text) throws Exception {
        KeyEvent[] keys = KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD).getEvents(text.toCharArray());
        if (keys != null) {
            for (KeyEvent key : keys) {
                automation.injectInputEvent(KeyEvent.changeTimeRepeat(key, SystemClock.uptimeMillis(), 0), true);
            }
            return new JSONObject();
        }
        AccessibilityNodeInfo root = automation.getRootInActiveWindow();
        AccessibilityNodeInfo field = root == null ? null : root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT);
        if (field == null || !field.isEditable()) {
            throw new IllegalStateException("no text field has the keyboard; tap one first");
        }
        CharSequence was = field.getText();
        String now = (was == null || field.isShowingHintText() ? "" : was.toString()) + text;
        Bundle arguments = new Bundle();
        arguments.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, now);
        if (!field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments)) {
            throw new IllegalStateException("the field would not take the text");
        }
        return new JSONObject();
    }

    private JSONObject screenshot(double scale, String format) throws Exception {
        Bitmap shot = automation.takeScreenshot();
        if (shot == null) {
            throw new IllegalStateException("the screen could not be read");
        }
        int width = Math.max(1, (int) Math.round(shot.getWidth() * scale));
        int height = Math.max(1, (int) Math.round(shot.getHeight() * scale));
        Bitmap sized = scale == 1 ? shot : Bitmap.createScaledBitmap(shot, width, height, true);
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        boolean png = format.equals("png");
        sized.compress(png ? Bitmap.CompressFormat.PNG : Bitmap.CompressFormat.JPEG, 80, bytes);
        JSONObject reply = new JSONObject();
        reply.put(png ? "png" : "jpeg", Base64.encodeToString(bytes.toByteArray(), Base64.NO_WRAP));
        reply.put("width", width);
        reply.put("height", height);
        return reply;
    }
}
