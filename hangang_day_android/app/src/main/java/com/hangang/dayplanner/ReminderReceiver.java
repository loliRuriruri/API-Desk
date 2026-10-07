package com.hangang.dayplanner;

import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.os.Build;

public class ReminderReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        String channelId = "hangang_day_plan";
        NotificationManager nm = (NotificationManager) context.getSystemService(Context.NOTIFICATION_SERVICE);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel ch = new NotificationChannel(channelId, "한강 나들이", NotificationManager.IMPORTANCE_HIGH);
            ch.setDescription("한강버스 운항/혼잡 확인 알림");
            nm.createNotificationChannel(ch);
        }
        Intent open = new Intent(context, MainActivity.class);
        open.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent pi = PendingIntent.getActivity(context, 1, open, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        android.app.Notification.Builder b = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
                ? new android.app.Notification.Builder(context, channelId)
                : new android.app.Notification.Builder(context);
        b.setSmallIcon(android.R.drawable.ic_dialog_info)
         .setContentTitle("11:20 한강버스 확인")
         .setContentText("12:55 압구정편 운항·잔여좌석과 이동시간을 확인하고 A/B 플랜을 결정하세요.")
         .setContentIntent(pi)
         .setAutoCancel(true);
        nm.notify(927, b.build());
    }
}
