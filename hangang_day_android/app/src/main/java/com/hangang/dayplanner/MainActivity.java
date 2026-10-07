package com.hangang.dayplanner;

import android.Manifest;
import android.app.Activity;
import android.app.AlarmManager;
import android.app.PendingIntent;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.webkit.JavascriptInterface;
import android.webkit.WebChromeClient;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Toast;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.net.HttpURLConnection;
import java.net.URL;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.text.SimpleDateFormat;
import java.util.Calendar;
import java.util.Locale;
import java.util.TimeZone;

public class MainActivity extends Activity {
    private WebView webView;
    private SharedPreferences prefs;
    private static final int REQ_NOTIF = 1001;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        prefs = getSharedPreferences("settings", MODE_PRIVATE);
        webView = new WebView(this);
        setContentView(webView);
        WebSettings s = webView.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        s.setAllowFileAccess(true);
        s.setAllowContentAccess(false);
        s.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        webView.setWebChromeClient(new WebChromeClient());
        webView.setWebViewClient(new WebViewClient());
        webView.addJavascriptInterface(new Bridge(this), "Android");
        webView.loadUrl("file:///android_asset/index.html");
    }

    @Override
    public void onBackPressed() {
        if (webView != null && webView.canGoBack()) webView.goBack();
        else super.onBackPressed();
    }

    private void toast(String msg) {
        runOnUiThread(() -> Toast.makeText(this, msg, Toast.LENGTH_SHORT).show());
    }

    private void openUrlInternal(String url) {
        try {
            startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)));
        } catch (Exception e) {
            toast("열 수 없습니다: " + e.getMessage());
        }
    }

    public class Bridge {
        private final Context ctx;
        Bridge(Context c) { ctx = c; }

        @JavascriptInterface
        public String getKakaoKey() { return prefs.getString("kakao_rest_key", ""); }

        @JavascriptInterface
        public void saveKakaoKey(String key) {
            prefs.edit().putString("kakao_rest_key", key == null ? "" : key.trim()).apply();
            toast("카카오 REST API 키를 이 기기에 저장했습니다.");
        }

        @JavascriptInterface
        public void clearKakaoKey() {
            prefs.edit().remove("kakao_rest_key").apply();
            toast("API 키를 삭제했습니다.");
        }

        @JavascriptInterface
        public void openUrl(String url) { openUrlInternal(url); }

        @JavascriptInterface
        public void copyText(String label, String text) {
            ClipboardManager cm = (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
            cm.setPrimaryClip(ClipData.newPlainText(label, text));
            toast(label + " 복사됨");
        }

        @JavascriptInterface
        public void openKakaoT() {
            try {
                Intent launch = getPackageManager().getLaunchIntentForPackage("com.kakao.taxi");
                if (launch != null) {
                    launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                    startActivity(launch);
                } else {
                    openUrlInternal("https://play.google.com/store/apps/details?id=com.kakao.taxi");
                }
            } catch (ActivityNotFoundException e) {
                openUrlInternal("https://play.google.com/store/apps/details?id=com.kakao.taxi");
            }
        }

        @JavascriptInterface
        public void requestRoutes() {
            String key = getKakaoKey();
            if (key.isEmpty()) {
                eval("window.onRouteError('카카오 REST API 키를 먼저 설정하세요.');");
                return;
            }
            new Thread(() -> {
                try {
                    JSONObject out = new JSONObject();
                    out.put("checkedAt", new SimpleDateFormat("HH:mm:ss", Locale.KOREA).format(System.currentTimeMillis()));
                    out.put("toApgujeong", routeFor(key, "바오로 흑염소농장 사당", "한강버스 압구정 선착장"));
                    out.put("toRacepark", routeFor(key, "바오로 흑염소농장 사당", "렛츠런파크 서울"));
                    out.put("pierToSeouldal", routeFor(key, "한강버스 여의도 선착장", "서울달 여의도공원"));
                    out.put("pierTo63", routeFor(key, "한강버스 여의도 선착장", "63스퀘어"));
                    eval("window.onRouteResult(" + JSONObject.quote(out.toString()) + ");");
                } catch (Exception e) {
                    String m = e.getMessage() == null ? e.toString() : e.getMessage();
                    eval("window.onRouteError(" + JSONObject.quote(m) + ");");
                }
            }).start();
        }

        @JavascriptInterface
        public void setReminder() {
            if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
                requestPermissions(new String[]{Manifest.permission.POST_NOTIFICATIONS}, REQ_NOTIF);
            }
            try {
                Calendar cal = Calendar.getInstance(TimeZone.getTimeZone("Asia/Seoul"));
                cal.set(2026, Calendar.OCTOBER, 9, 11, 20, 0);
                cal.set(Calendar.MILLISECOND, 0);
                if (cal.getTimeInMillis() <= System.currentTimeMillis()) {
                    toast("2026년 10월 9일 11:20이 이미 지났습니다.");
                    return;
                }
                Intent i = new Intent(ctx, ReminderReceiver.class);
                PendingIntent pi = PendingIntent.getBroadcast(ctx, 927, i, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
                AlarmManager am = (AlarmManager) getSystemService(ALARM_SERVICE);
                am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, cal.getTimeInMillis(), pi);
                toast("10월 9일 11:20 확인 알림을 설정했습니다.");
            } catch (Exception e) {
                toast("알림 설정 실패: " + e.getMessage());
            }
        }

        private JSONObject routeFor(String key, String originQ, String destQ) throws Exception {
            JSONObject o = findPlace(key, originQ);
            JSONObject d = findPlace(key, destQ);
            String origin = o.getString("x") + "," + o.getString("y");
            String dest = d.getString("x") + "," + d.getString("y");
            String u = "https://apis-navi.kakaomobility.com/v1/directions?origin=" + enc(origin) +
                    "&destination=" + enc(dest) + "&priority=RECOMMEND&summary=true";
            JSONObject root = new JSONObject(httpGet(u, key));
            JSONArray routes = root.optJSONArray("routes");
            if (routes == null || routes.length() == 0) throw new Exception("길찾기 결과가 없습니다: " + destQ);
            JSONObject summary = routes.getJSONObject(0).optJSONObject("summary");
            if (summary == null) throw new Exception("길찾기 응답 형식 오류");
            JSONObject fare = summary.optJSONObject("fare");
            JSONObject r = new JSONObject();
            r.put("origin", originQ);
            r.put("destination", destQ);
            r.put("durationSec", summary.optInt("duration", 0));
            r.put("distanceM", summary.optInt("distance", 0));
            r.put("taxiFare", fare == null ? 0 : fare.optInt("taxi", 0));
            r.put("originName", o.optString("place_name", originQ));
            r.put("destinationName", d.optString("place_name", destQ));
            return r;
        }

        private JSONObject findPlace(String key, String query) throws Exception {
            String url = "https://dapi.kakao.com/v2/local/search/keyword.json?query=" + enc(query) + "&size=5";
            JSONObject root = new JSONObject(httpGet(url, key));
            JSONArray docs = root.optJSONArray("documents");
            if (docs == null || docs.length() == 0) {
                String alias = query;
                if (query.contains("바오로")) alias = "바오로 흑염소";
                else if (query.contains("압구정")) alias = "압구정 한강버스";
                else if (query.contains("여의도 선착장")) alias = "여의도 한강버스 선착장";
                else if (query.contains("서울달")) alias = "서울달";
                url = "https://dapi.kakao.com/v2/local/search/keyword.json?query=" + enc(alias) + "&size=5";
                root = new JSONObject(httpGet(url, key));
                docs = root.optJSONArray("documents");
            }
            if (docs == null || docs.length() == 0) throw new Exception("장소를 찾지 못했습니다: " + query);
            return docs.getJSONObject(0);
        }

        private String httpGet(String urlStr, String key) throws Exception {
            HttpURLConnection c = (HttpURLConnection) new URL(urlStr).openConnection();
            c.setRequestMethod("GET");
            c.setConnectTimeout(10000);
            c.setReadTimeout(12000);
            c.setRequestProperty("Authorization", "KakaoAK " + key);
            c.setRequestProperty("Accept", "application/json");
            int code = c.getResponseCode();
            InputStream is = code >= 200 && code < 300 ? c.getInputStream() : c.getErrorStream();
            BufferedReader br = new BufferedReader(new InputStreamReader(is, StandardCharsets.UTF_8));
            StringBuilder sb = new StringBuilder();
            String line;
            while ((line = br.readLine()) != null) sb.append(line);
            br.close(); c.disconnect();
            if (code < 200 || code >= 300) throw new Exception("API 오류 " + code + ": " + sb);
            return sb.toString();
        }

        private String enc(String s) { return URLEncoder.encode(s, StandardCharsets.UTF_8); }
        private void eval(String js) { runOnUiThread(() -> webView.evaluateJavascript(js, null)); }
    }
}
