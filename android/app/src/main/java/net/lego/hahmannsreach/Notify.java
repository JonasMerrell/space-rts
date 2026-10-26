package net.lego.hahmannsreach;

import android.Manifest;
import android.app.Activity;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.job.JobInfo;
import android.app.job.JobScheduler;
import android.content.ComponentName;
import android.content.Context;
import android.content.pm.PackageManager;
import android.os.Build;

/** Phone notifications: channels, the periodic server poll (NotifyJob) and the "seen up to" clock. */
public final class Notify {
    static final String ALERTS = "alerts";
    static final String NEWS = "news";
    static final int JOB_ID = 7777;
    static final long PERIOD_MS = 15 * 60 * 1000L; // JobScheduler's minimum

    /** True while the game is on screen: the in-game toasts cover it, so the poller stays quiet. */
    static volatile boolean foreground = false;

    private Notify() {}

    /** Called from MainActivity.onCreate. */
    public static void setup(Activity a) {
        try {
            channels(a);
            schedule(a);
        } catch (RuntimeException e) {
            // notifications are a nicety; never let them stop the game from starting
            android.util.Log.w(NotifyJob.TAG, "notification setup failed: " + e);
        }
        if (Build.VERSION.SDK_INT >= 33
                && a.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            a.requestPermissions(new String[] {Manifest.permission.POST_NOTIFICATIONS}, 1);
        }
    }

    static void channels(Context c) {
        NotificationManager nm = c.getSystemService(NotificationManager.class);
        NotificationChannel alerts = new NotificationChannel(ALERTS, "Attacks and losses", NotificationManager.IMPORTANCE_HIGH);
        alerts.setDescription("Under attack, ships destroyed, planets lost");
        NotificationChannel news = new NotificationChannel(NEWS, "Fleet news", NotificationManager.IMPORTANCE_DEFAULT);
        news.setDescription("Fleets arriving, planets captured, ships built");
        nm.createNotificationChannel(alerts);
        nm.createNotificationChannel(news);
    }

    static void schedule(Context c) {
        JobScheduler js = c.getSystemService(JobScheduler.class);
        if (js.getPendingJob(JOB_ID) != null) return;
        JobInfo job = new JobInfo.Builder(JOB_ID, new ComponentName(c, NotifyJob.class))
                .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
                .setPeriodic(PERIOD_MS)
                .setPersisted(true) // survives reboots (RECEIVE_BOOT_COMPLETED)
                .build();
        js.schedule(job);
    }

    /** Everything up to now was seen in the app; only notify about what happens after. */
    static void markSeen(Context c) {
        c.getSharedPreferences("notify", Context.MODE_PRIVATE).edit()
                .putString("since", Double.toString(System.currentTimeMillis() / 1000.0)).apply();
    }

    static double since(Context c) {
        String s = c.getSharedPreferences("notify", Context.MODE_PRIVATE).getString("since", null);
        return s == null ? System.currentTimeMillis() / 1000.0 : Double.parseDouble(s);
    }
}
