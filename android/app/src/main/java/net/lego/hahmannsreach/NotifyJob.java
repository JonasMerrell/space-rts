package net.lego.hahmannsreach;

import android.app.Notification;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.job.JobParameters;
import android.app.job.JobService;
import android.content.Context;
import android.content.Intent;
import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.File;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

/**
 * Every ~15 min: log in to the game server with the account the app saved (files/notify.json, written by
 * the Rust client on login), ask for my events since the last check, and post them as notifications.
 * Same JSON-lines protocol as the game: Hello -> Welcome, Events{since} -> Events[[code, unix_time, event]].
 */
public class NotifyJob extends JobService {
    static final String TAG = "HRNotify";

    @Override
    public boolean onStartJob(JobParameters params) {
        new Thread(() -> {
            try {
                poll(this);
            } catch (Exception e) {
                Log.w(TAG, "poll failed: " + e);
            }
            jobFinished(params, false);
        }).start();
        return true;
    }

    @Override
    public boolean onStopJob(JobParameters params) {
        return false; // the next period will try again
    }

    static void poll(Context c) throws Exception {
        File f = new File(c.getFilesDir(), "notify.json");
        if (!f.exists()) return; // logged out
        JSONObject login = new JSONObject(new String(Files.readAllBytes(f.toPath()), StandardCharsets.UTF_8));
        String[] hp = login.getString("server").split(":");
        int port = hp.length > 1 ? Integer.parseInt(hp[1]) : 7777;
        double since = Notify.since(c);

        try (Socket s = new Socket()) {
            s.connect(new InetSocketAddress(hp[0], port), 10_000);
            s.setSoTimeout(15_000);
            OutputStream out = s.getOutputStream();
            BufferedReader in = new BufferedReader(new InputStreamReader(s.getInputStream(), StandardCharsets.UTF_8));
            JSONObject hello = new JSONObject().put("user", login.getString("user"))
                    .put("pass", login.getString("pass")).put("register", false);
            send(out, new JSONObject().put("Hello", hello));
            send(out, new JSONObject().put("Events", new JSONObject().put("since", since)));
            String line;
            while ((line = in.readLine()) != null) {
                JSONObject m = new JSONObject(line);
                if (m.has("Error")) {
                    Log.w(TAG, "server: " + m.getJSONObject("Error").optString("msg"));
                    return;
                }
                if (m.has("Events")) {
                    show(c, m.getJSONArray("Events"), since);
                    return;
                }
            }
        }
    }

    static void send(OutputStream out, JSONObject m) throws Exception {
        out.write((m.toString() + "\n").getBytes(StandardCharsets.UTF_8));
        out.flush();
    }

    static void show(Context c, JSONArray events, double since) {
        double newest = since;
        NotificationManager nm = c.getSystemService(NotificationManager.class);
        Intent open = new Intent(c, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        PendingIntent tap = PendingIntent.getActivity(c, 0, open, PendingIntent.FLAG_IMMUTABLE);
        int posted = 0;
        for (int i = 0; i < events.length(); i++) {
            JSONArray row = events.optJSONArray(i);
            if (row == null) continue;
            String code = row.optString(0);
            double when = row.optDouble(1);
            JSONObject e = row.optJSONObject(2);
            newest = Math.max(newest, when);
            if (e == null || !worthNotifying(e) || Notify.foreground) continue;
            String kind = e.optString("kind");
            boolean alert = kind.equals("Battle") || kind.equals("Bad");
            Notification n = new Notification.Builder(c, alert ? Notify.ALERTS : Notify.NEWS)
                    .setSmallIcon(R.drawable.ic_notify)
                    .setContentTitle(e.optString("title"))
                    .setContentText(e.optString("body"))
                    .setSubText(code)
                    .setWhen((long) (when * 1000))
                    .setShowWhen(true)
                    .setColor(alert ? 0xFFE63946 : 0xFF4CC9F0)
                    .setGroup("events")
                    .setContentIntent(tap)
                    .setAutoCancel(true)
                    .build();
            nm.notify((code + "#" + e.optLong("seq")).hashCode(), n);
            posted++;
        }
        if (posted > 1) {
            // Android bundles same-group notifications under a summary
            nm.notify("summary".hashCode(), new Notification.Builder(c, Notify.NEWS)
                    .setSmallIcon(R.drawable.ic_notify)
                    .setContentTitle("Hahmann's Reach")
                    .setContentText(posted + " updates")
                    .setGroupSummary(true)
                    .setGroup("events")
                    .setContentIntent(tap)
                    .setAutoCancel(true)
                    .build());
        }
        Log.i(TAG, events.length() + " events, " + posted + " notified");
        c.getSharedPreferences("notify", Context.MODE_PRIVATE).edit()
                .putString("since", Double.toString(newest)).apply();
    }

    /** Arrivals, attacks, captures/losses, builds; skip routine chatter like "Departed". */
    static boolean worthNotifying(JSONObject e) {
        String kind = e.optString("kind");
        if (kind.equals("Info")) return e.isNull("to"); // game started / extended / over
        return !e.optString("title").equals("Retreating");
    }
}
