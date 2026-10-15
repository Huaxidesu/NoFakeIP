// 窄屏界面校验用的「假 Tauri 后端」。
//
// 为什么需要：前端要连上 Rust 才能拿到数据，而自动化校验跑在普通浏览器里，
// 没有 IPC。这里在页面脚本之前注入一个最小的 __TAURI_INTERNALS__ 实现，
// 返回可控的假数据 —— 这样「测速结果卡片」「切页面不中断任务」这类
// 必须真的有数据才能验证的行为，才能在 CI/本地自动跑。
//
// 长任务故意加了延时（__delay），用来制造"任务进行中"的窗口。
//
// 由 tools/verify-mobile-ui.py 通过 Page.addScriptToEvaluateOnNewDocument 注入。

(() => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const now = new Date().toISOString();

  const mkServer = (i, name, protocol, region, address) => ({
    server_id: i,
    server_name: name,
    protocol,
    region,
    address,
    ok: true,
    error: null,
    samples: 3,
    min_ms: 8 + i * 4,
    avg_ms: 12 + i * 9,
    max_ms: 30 + i * 10,
    p95_ms: 25 + i * 9,
    connect_ms: 3 + i,
    tls_ms: protocol === "dot" || protocol === "doh" ? 7 + i : 0,
    query_ms: 2 + i,
    answers: [`1.2.3.${i}`, `5.6.7.${i}`],
    ttl: 60 + i,
    rcode: 0,
    score: 100 - i * 5,
    geo: [],
  });

  const BENCH = [
    {
      domain: "github.com",
      region: "global",
      confidence: 0.9,
      results: [
        mkServer(1, "阿里 DNS", "udp", "china", "223.5.5.5"),
        mkServer(2, "DNSPod", "udp", "china", "119.29.29.29"),
        mkServer(3, "Cloudflare", "doh", "global", "https://cloudflare-dns.com/dns-query"),
        mkServer(4, "Google DNS", "dot", "global", "tls://8.8.8.8:853"),
      ],
      best_china: "阿里 DNS",
      best_global: "Cloudflare",
      recommended: "阿里 DNS",
      batch_id: "b1",
      created_at: now,
    },
  ];

  const REC = {
    china: {
      name: "阿里 DNS", address: "223.5.5.5", protocol: "udp", region: "china",
      avg_ms: 18.4, score: 92, success_rate: 1, samples: 3,
    },
    foreign: {
      name: "Cloudflare", address: "https://cloudflare-dns.com/dns-query", protocol: "doh",
      region: "global", avg_ms: 52.1, score: 88, success_rate: 1, samples: 3,
    },
    reasons: ["阿里 DNS 在国内样本上平均延迟最低", "Cloudflare 解析国外域名干净，未发现污染"],
    china_avg_ms: 18.4,
    foreign_avg_ms: 52.1,
    pollution_rate: 0.06,
    generated_at: now,
    alternates: [],
  };

  const SERVERS = [
    { id: 1, name: "阿里 DNS", protocol: "udp", address: "223.5.5.5", host: null, bootstrap_ip: null, region: "china", enabled: true, builtin: true, tags: [], remark: null },
    { id: 2, name: "DNSPod", protocol: "udp", address: "119.29.29.29", host: null, bootstrap_ip: null, region: "china", enabled: true, builtin: true, tags: [], remark: null },
    { id: 3, name: "Cloudflare", protocol: "doh", address: "https://cloudflare-dns.com/dns-query", host: "cloudflare-dns.com", bootstrap_ip: null, region: "global", enabled: true, builtin: true, tags: [], remark: null },
    { id: 4, name: "Google DNS", protocol: "dot", address: "tls://8.8.8.8:853", host: "dns.google", bootstrap_ip: null, region: "global", enabled: true, builtin: true, tags: [], remark: null },
  ];

  const STATUS = {
    version: "0.1.0",
    data_dir: "C:\\Users\\demo\\AppData\\Roaming\\com.nofakeip.desktop",
    server_count: 4,
    enabled_count: 4,
    last_bench_at: now,
    stats: { servers: 4, benchmarks: 12, domains: 37, rules: 110000, schema_version: 1, path: "nofakeip.db" },
    recommendation: REC,
  };

  const SETTINGS = {
    timeout_secs: 5,
    rounds: 3,
    concurrency: 16,
    fake_ip_range: "198.18.0.0/15",
    online_geo: false,
    sample_domains: ["github.com", "youtube.com"],
  };

  const DIAG = {
    domain: "github.com",
    classification: {
      domain: "github.com", region: "global", confidence: 0.93, source: "local_rule",
      matched_rule: "github.com", registrable: "github.com", tld: "com",
    },
    bench: BENCH[0],
    report: {
      domain: "github.com", region: "global", risk: "low", fake_ip: "normal",
      consistency: 1.0, fake_ip_hits: [], poisoned_ips: [], findings: [], observations: [],
    },
    recommendation: REC,
  };

  const FULL = { results: BENCH, reports: [], recommendation: REC, elapsed_ms: 4200 };

  // 长任务用 __delay 制造"进行中"的窗口
  const RESP = {
    get_status: STATUS,
    list_servers: SERVERS,
    get_settings: SETTINGS,
    get_recommendation: REC,
    get_data_dir: STATUS.data_dir,
    list_history: [],
    list_benchmarks: [],
    list_data_sources: [],
    run_benchmark: { __delay: 1500, __value: BENCH },
    run_full_benchmark: { __delay: 1800, __value: FULL },
    detect_domain: { __delay: 1500, __value: DIAG },
    "plugin:event|listen": 1,
    "plugin:event|unlisten": null,
  };

  let seq = 0;
  window.__TAURI_INTERNALS__ = {
    invoke: (cmd) => {
      const r = RESP[cmd];
      if (r && typeof r === "object" && "__delay" in r) {
        return sleep(r.__delay).then(() => r.__value);
      }
      if (r !== undefined) return Promise.resolve(r);
      return Promise.reject(new Error("stub: 未实现的命令 " + cmd));
    },
    transformCallback: (cb) => {
      const id = ++seq;
      window["_" + id] = cb;
      return id;
    },
    unregisterCallback: (id) => {
      delete window["_" + id];
    },
    convertFileSrc: (p) => p,
  };
  window.isTauri = true;
})();
