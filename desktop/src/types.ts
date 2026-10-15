// 与 Rust 端模型一一对应的类型定义。
// 契约见 doc/02-核心引擎设计.md 与 doc/08-GUI规格.md 第 3 节。
// 约定：字段名保持 snake_case，与 serde 默认输出一致，不做驼峰转换（减少映射错误）。

export type DnsProtocol = "udp" | "tcp" | "dot" | "doh";
export type Region = "china" | "global" | "unknown";
export type RiskLevel = "low" | "medium" | "high";
export type FakeIpStatus = "normal" | "detected" | "suspicious" | "unknown";
export type RuleSource =
  | "user_override"
  | "local_rule"
  | "suffix_rule"
  | "ip_geo"
  | "default";

export interface DnsServer {
  id: number;
  name: string;
  protocol: DnsProtocol;
  address: string;
  host?: string | null;
  bootstrap_ip?: string | null;
  region: Region;
  enabled: boolean;
  builtin: boolean;
  tags: string[];
  remark?: string | null;
}

export interface IpGeo {
  ip: string;
  country_code?: string | null;
  country?: string | null;
  asn?: number | null;
  as_name?: string | null;
  is_china: boolean;
  is_private: boolean;
  is_reserved: boolean;
  source: "local" | "online" | "unknown";
}

export interface ServerBenchResult {
  server_id: number;
  server_name: string;
  protocol: DnsProtocol;
  region: Region;
  address: string;
  ok: boolean;
  error?: string | null;
  samples: number;
  min_ms: number;
  avg_ms: number;
  max_ms: number;
  p95_ms: number;
  connect_ms: number;
  tls_ms: number;
  query_ms: number;
  answers: string[];
  ttl?: number | null;
  rcode?: number | null;
  score: number;
  geo: IpGeo[];
}

export interface DomainBenchResult {
  domain: string;
  region: Region;
  confidence: number;
  results: ServerBenchResult[];
  best_china?: string | null;
  best_global?: string | null;
  recommended?: string | null;
  batch_id?: string | null;
  created_at: string;
}

export interface Classification {
  domain: string;
  region: Region;
  confidence: number;
  source: RuleSource;
  matched_rule?: string | null;
  registrable?: string | null;
  tld?: string | null;
}

export interface Finding {
  code: string;
  severity: RiskLevel;
  message: string;
  evidence: string[];
}

export interface ServerObservation {
  server_name: string;
  region: Region;
  ok: boolean;
  ips: string[];
  ttl?: number | null;
  rcode?: number | null;
  error?: string | null;
  is_fake_ip: boolean;
}

export interface PollutionReport {
  domain: string;
  region: Region;
  risk: RiskLevel;
  fake_ip: FakeIpStatus;
  consistency?: number | null;
  fake_ip_hits: string[];
  poisoned_ips: string[];
  findings: Finding[];
  observations: ServerObservation[];
}

export interface RecommendedServer {
  name: string;
  address: string;
  protocol: DnsProtocol;
  region: Region;
  avg_ms: number;
  score: number;
  success_rate: number;
  samples: number;
}

export interface Recommendation {
  china?: RecommendedServer | null;
  foreign?: RecommendedServer | null;
  reasons: string[];
  china_avg_ms: number;
  foreign_avg_ms: number;
  pollution_rate: number;
  generated_at: string;
  alternates: RecommendedServer[];
}

export interface DomainDiagnosis {
  domain: string;
  classification: Classification;
  bench: DomainBenchResult;
  report: PollutionReport;
  recommendation: Recommendation;
}

export interface DomainHistoryRow {
  domain: string;
  region: Region;
  risk?: RiskLevel | null;
  fake_ip?: string | null;
  last_ips: string[];
  query_count: number;
  last_seen_at: string;
  first_seen_at: string;
}

export interface DataStatus {
  id: string;
  name: string;
  target_file: string;
  exists: boolean;
  size_bytes: number;
  entries: number;
  modified_at?: string | null;
  using_builtin: boolean;
}

export interface UpdateReport {
  id: string;
  name: string;
  ok: boolean;
  url_used?: string | null;
  attempts: number;
  bytes: number;
  entries: number;
  target?: string | null;
  error?: string | null;
  duration_ms: number;
}

export interface StorageStats {
  servers: number;
  benchmarks: number;
  domains: number;
  rules: number;
  schema_version: number;
  path?: string | null;
}

export interface AppStatus {
  version: string;
  data_dir: string;
  server_count: number;
  enabled_count: number;
  last_bench_at?: string | null;
  stats: StorageStats;
  /** 上次全量测速得到的推荐（若有） */
  recommendation?: Recommendation | null;
}

export interface FullBenchReport {
  results: DomainBenchResult[];
  reports: PollutionReport[];
  recommendation: Recommendation;
  elapsed_ms: number;
}

export interface BenchOptions {
  rounds: number;
  timeout_secs: number;
  concurrency: number;
  domains?: string[];
}

export interface Settings {
  timeout_secs: number;
  rounds: number;
  concurrency: number;
  fake_ip_range: string;
  online_geo: boolean;
  sample_domains: string[];
  /** 出站代理模式："auto" / "custom" / "off" */
  proxy_mode: string;
  /** 手动模式下的代理地址 */
  proxy_url: string;
}

/** 出站代理当前状态 */
export interface ProxyStatus {
  mode: string;
  custom_url: string;
  resolved_url: string | null;
  enabled: boolean;
  effective_url: string | null;
}

export interface BenchProgress {
  done: number;
  total: number;
  current_domain: string;
}

export interface DataProgress {
  done: number;
  total: number;
  name: string;
}

// —— 展示辅助 ——

export const REGION_LABEL: Record<Region, string> = {
  china: "国内",
  global: "国外",
  unknown: "未知",
};

export const RISK_LABEL: Record<RiskLevel, string> = {
  low: "低",
  medium: "中",
  high: "高",
};

export const FAKE_IP_LABEL: Record<FakeIpStatus, string> = {
  normal: "正常",
  detected: "已生效",
  suspicious: "可疑",
  unknown: "未知",
};

export const SOURCE_LABEL: Record<RuleSource, string> = {
  user_override: "用户覆盖",
  local_rule: "规则库",
  suffix_rule: "TLD 规则",
  ip_geo: "IP 归属地",
  default: "默认",
};

export const PROTOCOL_LABEL: Record<DnsProtocol, string> = {
  udp: "UDP",
  tcp: "TCP",
  dot: "DoT",
  doh: "DoH",
};
