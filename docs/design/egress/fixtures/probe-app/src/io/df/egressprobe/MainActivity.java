// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2025-2026 Seyud
package io.df.egressprobe;

import android.app.Activity;
import android.os.Bundle;
import android.os.StrictMode;
import android.util.Log;
import android.widget.TextView;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.OutputStreamWriter;
import java.net.HttpURLConnection;
import java.net.Proxy;
import java.net.ProxySelector;
import java.net.Socket;
import java.net.URI;
import java.net.URL;
import java.util.Properties;

/**
 * t15 复验探针：
 *  - 打印进程内 java.net 代理属性（凭据只报「是否已设置」）与 ProxySelector 的选择结果；
 *  - 用 HttpURLConnection 请求 http://app.fixture/ —— fixture 侧按监听端口归因，
 *    只有真走了「应用级代理端口」才会在该端口日志里出现 app.fixture；
 *  - 用裸 Socket 连 198.51.100.9:80 —— socksProxyHost 生效时会经 SOCKS5 建立连接，
 *    fixture 会记录 CONNECT 与隧道内请求。
 * 这是 debug 探针，允许主线程联网（StrictMode.permitNetwork）。
 */
public class MainActivity extends Activity {
    private static final String TAG = "EGRESSPROBE";
    private static final String[] KEYS = {
            "http.proxyHost", "http.proxyPort",
            "https.proxyHost", "https.proxyPort",
            "socksProxyHost", "socksProxyPort",
            "java.net.socks.username", "java.net.socks.password",
            "java.net.useSystemProxies",
    };

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        StrictMode.setThreadPolicy(
                new StrictMode.ThreadPolicy.Builder().permitNetwork().build());

        String report = buildReport()
                + "fetch.app=" + fetch("http://app.fixture/") + "\n"
                + "fetch.ip=" + fetch("http://198.51.100.7/") + "\n"
                + "rawsocks=" + rawSocket("198.51.100.9", 80) + "\n"
                + "socksdomain=" + socksDomain("app.fixture", 80) + "\n";
        Log.i(TAG, report);
        persist(report);
        TextView view = new TextView(this);
        view.setText(report);
        setContentView(view);
    }

    private String buildReport() {
        Properties props = System.getProperties();
        StringBuilder sb = new StringBuilder();
        sb.append("pid=").append(android.os.Process.myPid()).append("\n");
        sb.append("package=").append(getPackageName()).append("\n");
        for (String key : KEYS) {
            String value = props.getProperty(key);
            if (key.endsWith("password")) {
                sb.append(key).append("=<set:").append(value != null && !value.isEmpty()).append(">\n");
            } else {
                sb.append(key).append('=').append(value == null ? "<null>" : value).append("\n");
            }
        }
        ProxySelector selector = ProxySelector.getDefault();
        sb.append("ProxySelector=").append(selector == null ? "<null>" : selector.getClass().getName()).append("\n");
        try {
            Proxy proxy = selector == null ? null
                    : selector.select(URI.create("http://app.fixture/")).get(0);
            sb.append("selector.app.fixture=").append(proxy == null ? "<null>" : proxy.toString()).append("\n");
        } catch (Throwable t) {
            sb.append("selector.app.fixture=<error:").append(t.getClass().getSimpleName()).append(">\n");
        }
        return sb.toString();
    }

    /** HttpURLConnection 走默认链路请求一次，返回状态码与前 64 字节或异常名。 */
    private String fetch(String url) {
        HttpURLConnection connection = null;
        try {
            connection = (HttpURLConnection) new URL(url).openConnection();
            connection.setConnectTimeout(3000);
            connection.setReadTimeout(3000);
            connection.setInstanceFollowRedirects(false);
            int code = connection.getResponseCode();
            InputStream stream = code >= 400 ? connection.getErrorStream() : connection.getInputStream();
            byte[] buffer = new byte[64];
            int read = stream == null ? 0 : stream.read(buffer);
            String body = read > 0 ? new String(buffer, 0, read).trim() : "";
            return "status=" + code + " body=" + body;
        } catch (Throwable t) {
            return "error=" + t.getClass().getSimpleName();
        } finally {
            if (connection != null) {
                connection.disconnect();
            }
        }
    }

    /** 裸 Socket：socksProxyHost/Port 生效时 JDK 会先和 SOCKS5 代理建链。 */
    private String rawSocket(String host, int port) {
        Socket socket = null;
        try {
            socket = new Socket(host, port);
            socket.setSoTimeout(3000);
            OutputStream out = socket.getOutputStream();
            out.write(("GET / HTTP/1.0\r\nHost: " + host + "\r\n\r\n").getBytes("UTF-8"));
            out.flush();
            byte[] buffer = new byte[96];
            int read = socket.getInputStream().read(buffer);
            String body = read > 0 ? new String(buffer, 0, read).trim() : "";
            return "ok body=" + body;
        } catch (Throwable t) {
            return "error=" + t.getClass().getSimpleName();
        } finally {
            if (socket != null) {
                try {
                    socket.close();
                } catch (Throwable ignored) {
                    // 探针，忽略
                }
            }
        }
    }

    /**
     * t9 用的远程 DNS 判据：目标地址用 createUnresolved，SOCKS 代理若生效，
     * 握手里的 ATYP 必须是域名（fixture 侧看到 app.fixture 而不是解析后的 IP）。
     */
    private String socksDomain(String host, int port) {
        Socket socket = null;
        try {
            socket = new Socket();
            socket.connect(java.net.InetSocketAddress.createUnresolved(host, port), 3000);
            socket.setSoTimeout(3000);
            OutputStream out = socket.getOutputStream();
            out.write(("GET / HTTP/1.1\r\nHost: " + host + "\r\n\r\n").getBytes("UTF-8"));
            out.flush();
            byte[] buffer = new byte[96];
            int read = socket.getInputStream().read(buffer);
            String body = read > 0 ? new String(buffer, 0, read).trim() : "";
            return "ok body=" + body;
        } catch (Throwable t) {
            return "error=" + t.getClass().getSimpleName();
        } finally {
            if (socket != null) {
                try {
                    socket.close();
                } catch (Throwable ignored) {
                    // 探针，忽略
                }
            }
        }
    }

    private void persist(String report) {
        File file = new File(getExternalFilesDir(null), "probe.txt");
        try (FileOutputStream out = new FileOutputStream(file);
             OutputStreamWriter writer = new OutputStreamWriter(out, "UTF-8")) {
            writer.write(report);
        } catch (Throwable t) {
            Log.w(TAG, "persist failed " + t.getClass().getSimpleName());
        }
    }
}
