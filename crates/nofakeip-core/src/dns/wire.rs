//! DNS 报文编解码（自研实现）。
//!
//! 为什么不依赖 `hickory-proto`：见 `doc/01-系统架构.md` 第 3 节。
//! 额外收益：畸形响应本身是污染/劫持的信号，自研可以精确识别并记录。

use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// 记录类型常量。
pub const TYPE_A: u16 = 1;
pub const TYPE_NS: u16 = 2;
pub const TYPE_CNAME: u16 = 5;
pub const TYPE_SOA: u16 = 6;
pub const TYPE_PTR: u16 = 12;
pub const TYPE_MX: u16 = 15;
pub const TYPE_TXT: u16 = 16;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_SRV: u16 = 33;
pub const TYPE_OPT: u16 = 41;
pub const TYPE_HTTPS: u16 = 65;
pub const TYPE_CAA: u16 = 257;

/// RCODE 常量。
pub const RCODE_NOERROR: u8 = 0;
pub const RCODE_FORMERR: u8 = 1;
pub const RCODE_SERVFAIL: u8 = 2;
pub const RCODE_NXDOMAIN: u8 = 3;
pub const RCODE_REFUSED: u8 = 5;

/// EDNS0 声明的 UDP 载荷大小（4096 是业界通用值）。
pub const EDNS_UDP_SIZE: u16 = 4096;

/// 把记录类型转成人类可读名称。
pub fn type_name(t: u16) -> &'static str {
    match t {
        TYPE_A => "A",
        TYPE_NS => "NS",
        TYPE_CNAME => "CNAME",
        TYPE_SOA => "SOA",
        TYPE_PTR => "PTR",
        TYPE_MX => "MX",
        TYPE_TXT => "TXT",
        TYPE_AAAA => "AAAA",
        TYPE_SRV => "SRV",
        TYPE_OPT => "OPT",
        TYPE_HTTPS => "HTTPS",
        TYPE_CAA => "CAA",
        _ => "UNKNOWN",
    }
}

/// 把记录类型字符串解析为数值。
pub fn parse_type(s: &str) -> Result<u16> {
    let up = s.trim().to_ascii_uppercase();
    Ok(match up.as_str() {
        "A" => TYPE_A,
        "NS" => TYPE_NS,
        "CNAME" => TYPE_CNAME,
        "SOA" => TYPE_SOA,
        "PTR" => TYPE_PTR,
        "MX" => TYPE_MX,
        "TXT" => TYPE_TXT,
        "AAAA" => TYPE_AAAA,
        "SRV" => TYPE_SRV,
        "HTTPS" => TYPE_HTTPS,
        "CAA" => TYPE_CAA,
        other => {
            return Err(CoreError::Protocol(format!("不支持的记录类型: {}", other)));
        }
    })
}

/// 问题段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
}

/// 资源记录的 RDATA。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RData {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    /// CNAME / NS / PTR
    Name(String),
    Txt(Vec<String>),
    Mx {
        preference: u16,
        exchange: String,
    },
    Soa {
        mname: String,
        rname: String,
        serial: u32,
        refresh: u32,
        retry: u32,
        expire: u32,
        minimum: u32,
    },
    Opt,
    Unknown {
        rtype: u16,
        raw_len: usize,
    },
}

impl RData {
    /// 取 IP（仅 A / AAAA）。
    pub fn ip(&self) -> Option<IpAddr> {
        match self {
            RData::A(v4) => Some(IpAddr::V4(*v4)),
            RData::Aaaa(v6) => Some(IpAddr::V6(*v6)),
            _ => None,
        }
    }
}

/// 资源记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    pub rtype: u16,
    pub rclass: u16,
    pub ttl: u32,
    pub rdata: RData,
}

/// 一条完整的 DNS 消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DnsMessage {
    pub id: u16,
    pub flags: u16,
    pub rcode: u8,
    pub truncated: bool,
    pub authoritative: bool,
    pub recursion_available: bool,
    pub questions: Vec<Question>,
    pub answers: Vec<Record>,
    pub authorities: Vec<Record>,
    pub additionals: Vec<Record>,
}

impl DnsMessage {
    /// 所有 A / AAAA 应答地址。
    pub fn ip_answers(&self) -> Vec<IpAddr> {
        self.answers.iter().filter_map(|r| r.rdata.ip()).collect()
    }

    /// CNAME 链（按应答顺序）。
    pub fn cname_chain(&self) -> Vec<String> {
        self.answers
            .iter()
            .filter(|r| r.rtype == TYPE_CNAME)
            .filter_map(|r| match &r.rdata {
                RData::Name(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }

    /// 最小 TTL（忽略 OPT 记录）。无有效记录时返回 None。
    pub fn min_ttl(&self) -> Option<u32> {
        self.answers
            .iter()
            .filter(|r| r.rtype != TYPE_OPT)
            .map(|r| r.ttl)
            .min()
    }

    /// 是否存在 NOERROR 但无 A/AAAA 应答（可能被 CNAME-only 解释）。
    pub fn has_no_ip_answer(&self) -> bool {
        self.ip_answers().is_empty()
    }

    pub fn is_nxdomain(&self) -> bool {
        self.rcode == RCODE_NXDOMAIN
    }

    pub fn is_noerror(&self) -> bool {
        self.rcode == RCODE_NOERROR
    }

    pub fn rcode_name(&self) -> &'static str {
        rcode_name(self.rcode)
    }
}

/// RCODE 名称。
pub fn rcode_name(rcode: u8) -> &'static str {
    match rcode {
        RCODE_NOERROR => "NOERROR",
        RCODE_FORMERR => "FORMERR",
        RCODE_SERVFAIL => "SERVFAIL",
        RCODE_NXDOMAIN => "NXDOMAIN",
        RCODE_REFUSED => "REFUSED",
        4 => "NOTIMP",
        6 => "YXDOMAIN",
        7 => "YXRRSET",
        8 => "NXRRSET",
        9 => "NOTAUTH",
        10 => "NOTZONE",
        _ => "OTHER",
    }
}

/// 校验域名是否可编码为 DNS 名。
pub fn validate_domain(domain: &str) -> Result<()> {
    let d = domain.trim().trim_end_matches('.');
    if d.is_empty() {
        return Err(CoreError::InvalidDomain("域名为空".into()));
    }
    if d.len() > 253 {
        return Err(CoreError::InvalidDomain(format!("域名过长 ({} 字节)", d.len())));
    }
    if d.split('.').any(|l| l.is_empty()) {
        return Err(CoreError::InvalidDomain(format!("域名含空标签: {}", d)));
    }
    for label in d.split('.') {
        if label.len() > 63 {
            return Err(CoreError::InvalidDomain(format!("标签过长: {}", label)));
        }
    }
    Ok(())
}

fn write_name(buf: &mut Vec<u8>, domain: &str) -> Result<()> {
    validate_domain(domain)?;
    let d = domain.trim().trim_end_matches('.');
    for label in d.split('.') {
        let bytes = label.as_bytes();
        buf.push(bytes.len() as u8);
        buf.extend_from_slice(bytes);
    }
    buf.push(0);
    Ok(())
}

/// 编码一条查询报文。`edns=true` 时附加 OPT(4096)。
pub fn encode_query(id: u16, domain: &str, qtype: u16, edns: bool) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&id.to_be_bytes());
    // RD=1
    buf.extend_from_slice(&0x0100u16.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    buf.extend_from_slice(&(if edns { 1u16 } else { 0u16 }).to_be_bytes()); // ARCOUNT

    write_name(&mut buf, domain)?;
    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes()); // IN

    if edns {
        buf.push(0); // 根域名
        buf.extend_from_slice(&TYPE_OPT.to_be_bytes());
        buf.extend_from_slice(&EDNS_UDP_SIZE.to_be_bytes()); // CLASS = UDP payload size
        buf.extend_from_slice(&0u32.to_be_bytes()); // TTL = extended rcode/version/flags
        buf.extend_from_slice(&0u16.to_be_bytes()); // RDLEN
    }
    Ok(buf)
}

/// 从报文头部读取事务 ID（用于 UDP 响应的伪造包校验）。
pub fn read_id(buf: &[u8]) -> u16 {
    if buf.len() < 2 {
        return 0;
    }
    u16::from_be_bytes([buf[0], buf[1]])
}

/// 读取（可能被压缩的）域名，返回名称与「继续解析的位置」。
fn read_name(buf: &[u8], start: usize) -> Result<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut pos = start;
    let mut next_pos = start;
    let mut jumped = false;
    let mut hops = 0usize;

    loop {
        if pos >= buf.len() {
            return Err(CoreError::Malformed("域名读取越界".into()));
        }
        let len = buf[pos];
        match len & 0xC0 {
            0x00 => {
                if len == 0 {
                    pos += 1;
                    if !jumped {
                        next_pos = pos;
                    }
                    break;
                }
                let l = len as usize;
                if pos + 1 + l > buf.len() {
                    return Err(CoreError::Malformed("标签越界".into()));
                }
                labels.push(String::from_utf8_lossy(&buf[pos + 1..pos + 1 + l]).into_owned());
                pos += 1 + l;
                if !jumped {
                    next_pos = pos;
                }
                if labels.len() > 128 {
                    return Err(CoreError::Malformed("标签数量异常".into()));
                }
            }
            0xC0 => {
                if pos + 1 >= buf.len() {
                    return Err(CoreError::Malformed("压缩指针不完整".into()));
                }
                let ptr = (((len as usize) & 0x3F) << 8) | buf[pos + 1] as usize;
                if !jumped {
                    next_pos = pos + 2;
                    jumped = true;
                }
                hops += 1;
                if hops > 16 {
                    return Err(CoreError::Malformed("压缩指针成环".into()));
                }
                if ptr >= buf.len() {
                    return Err(CoreError::Malformed("压缩指针越界".into()));
                }
                pos = ptr;
            }
            _ => return Err(CoreError::Malformed("非法标签长度前缀".into())),
        }
    }

    let name = if labels.is_empty() {
        ".".to_string()
    } else {
        labels.join(".")
    };
    Ok((name, next_pos))
}

fn read_u16(buf: &[u8], pos: usize) -> Result<u16> {
    if pos + 2 > buf.len() {
        return Err(CoreError::Malformed("读取 u16 越界".into()));
    }
    Ok(u16::from_be_bytes([buf[pos], buf[pos + 1]]))
}

fn read_u32(buf: &[u8], pos: usize) -> Result<u32> {
    if pos + 4 > buf.len() {
        return Err(CoreError::Malformed("读取 u32 越界".into()));
    }
    Ok(u32::from_be_bytes([
        buf[pos],
        buf[pos + 1],
        buf[pos + 2],
        buf[pos + 3],
    ]))
}

fn parse_rdata(buf: &[u8], rtype: u16, rd_start: usize, rd_len: usize) -> Result<RData> {
    let rd_end = rd_start + rd_len;
    if rd_end > buf.len() {
        return Err(CoreError::Malformed("RDATA 越界".into()));
    }
    Ok(match rtype {
        TYPE_A => {
            if rd_len != 4 {
                return Err(CoreError::Malformed(format!("A 记录长度异常: {}", rd_len)));
            }
            RData::A(Ipv4Addr::new(
                buf[rd_start],
                buf[rd_start + 1],
                buf[rd_start + 2],
                buf[rd_start + 3],
            ))
        }
        TYPE_AAAA => {
            if rd_len != 16 {
                return Err(CoreError::Malformed(format!("AAAA 记录长度异常: {}", rd_len)));
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&buf[rd_start..rd_start + 16]);
            RData::Aaaa(Ipv6Addr::from(octets))
        }
        TYPE_CNAME | TYPE_NS | TYPE_PTR => {
            let (name, _) = read_name(buf, rd_start)?;
            RData::Name(name)
        }
        TYPE_MX => {
            let pref = read_u16(buf, rd_start)?;
            let (exchange, _) = read_name(buf, rd_start + 2)?;
            RData::Mx {
                preference: pref,
                exchange,
            }
        }
        TYPE_TXT => {
            let mut out = Vec::new();
            let mut p = rd_start;
            while p < rd_end {
                let l = buf[p] as usize;
                if p + 1 + l > rd_end {
                    break;
                }
                out.push(String::from_utf8_lossy(&buf[p + 1..p + 1 + l]).into_owned());
                p += 1 + l;
            }
            RData::Txt(out)
        }
        TYPE_SOA => {
            let (mname, p1) = read_name(buf, rd_start)?;
            let (rname, p2) = read_name(buf, p1)?;
            RData::Soa {
                mname,
                rname,
                serial: read_u32(buf, p2)?,
                refresh: read_u32(buf, p2 + 4)?,
                retry: read_u32(buf, p2 + 8)?,
                expire: read_u32(buf, p2 + 12)?,
                minimum: read_u32(buf, p2 + 16)?,
            }
        }
        TYPE_OPT => RData::Opt,
        other => RData::Unknown {
            rtype: other,
            raw_len: rd_len,
        },
    })
}

fn read_record(buf: &[u8], pos: usize) -> Result<(Record, usize)> {
    let (name, mut p) = read_name(buf, pos)?;
    let rtype = read_u16(buf, p)?;
    let rclass = read_u16(buf, p + 2)?;
    let ttl = read_u32(buf, p + 4)?;
    let rd_len = read_u16(buf, p + 8)? as usize;
    p += 10;
    let rdata = parse_rdata(buf, rtype, p, rd_len)?;
    p += rd_len;
    if p > buf.len() {
        return Err(CoreError::Malformed("记录越界".into()));
    }
    Ok((
        Record {
            name,
            rtype,
            rclass,
            ttl,
            rdata,
        },
        p,
    ))
}

/// 解析一条 DNS 响应报文。
///
/// 带越界保护与压缩指针环检测；畸形报文返回 [`CoreError::Malformed`]。
pub fn parse_message(buf: &[u8]) -> Result<DnsMessage> {
    if buf.len() < 12 {
        return Err(CoreError::Malformed(format!("报文过短: {} 字节", buf.len())));
    }
    let id = read_u16(buf, 0)?;
    let flags = read_u16(buf, 2)?;
    let qd = read_u16(buf, 4)? as usize;
    let an = read_u16(buf, 6)? as usize;
    let ns = read_u16(buf, 8)? as usize;
    let ar = read_u16(buf, 10)? as usize;

    // 防御：计数与实际长度明显不符时直接拒绝，避免构造超大分配
    let total = qd + an + ns + ar;
    if total > 4096 {
        return Err(CoreError::Malformed(format!("记录数异常: {}", total)));
    }

    let mut pos = 12usize;
    let mut questions = Vec::with_capacity(qd.min(16));
    for _ in 0..qd {
        let (name, p) = read_name(buf, pos)?;
        let qtype = read_u16(buf, p)?;
        let qclass = read_u16(buf, p + 2)?;
        questions.push(Question {
            name,
            qtype,
            qclass,
        });
        pos = p + 4;
    }

    let read_section = |count: usize, pos: &mut usize| -> Result<Vec<Record>> {
        let mut out = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            let (r, p) = read_record(buf, *pos)?;
            out.push(r);
            *pos = p;
        }
        Ok(out)
    };

    let answers = read_section(an, &mut pos)?;
    let authorities = read_section(ns, &mut pos)?;
    let additionals = read_section(ar, &mut pos)?;

    Ok(DnsMessage {
        id,
        flags,
        rcode: (flags & 0x000F) as u8,
        truncated: flags & 0x0200 != 0,
        authoritative: flags & 0x0400 != 0,
        recursion_available: flags & 0x0080 != 0,
        questions,
        answers,
        authorities,
        additionals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_domain_rules() {
        assert!(validate_domain("google.com").is_ok());
        assert!(validate_domain("a.b.c.d.example.com").is_ok());
        assert!(validate_domain("google.com.").is_ok());
        assert!(validate_domain("").is_err());
        assert!(validate_domain("a..b").is_err());
        let long_label = "a".repeat(64);
        assert!(validate_domain(&format!("{}.com", long_label)).is_err());
    }

    #[test]
    fn encode_query_layout() {
        let q = encode_query(0x1234, "google.com", TYPE_A, true).unwrap();
        assert_eq!(&q[0..2], &[0x12, 0x34]);
        assert_eq!(&q[2..4], &[0x01, 0x00]); // RD
        assert_eq!(&q[4..6], &[0x00, 0x01]); // QDCOUNT
        assert_eq!(&q[10..12], &[0x00, 0x01]); // ARCOUNT (EDNS)
        // 域名编码：6google3com0
        assert_eq!(&q[12..13], &[6]);
        assert_eq!(&q[13..19], b"google");
        assert_eq!(&q[19..20], &[3]);
        assert_eq!(&q[20..23], b"com");
        assert_eq!(&q[23..24], &[0]);
        assert_eq!(&q[24..26], &TYPE_A.to_be_bytes());
        assert_eq!(&q[26..28], &[0x00, 0x01]);
        // OPT 记录
        assert_eq!(&q[28..29], &[0]);
        assert_eq!(&q[29..31], &TYPE_OPT.to_be_bytes());
    }

    #[test]
    fn encode_query_without_edns() {
        let q = encode_query(1, "a.com", TYPE_AAAA, false).unwrap();
        assert_eq!(&q[10..12], &[0x00, 0x00]);
        // 12(头部) + 7(名字: 1a + 3com + 0) + 2(qtype) + 2(qclass) = 23
        assert_eq!(q.len(), 23);
    }

    /// 构造一条带压缩指针的响应，验证解析正确。
    fn build_response_with_pointer() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0xABCDu16.to_be_bytes()); // id
        b.extend_from_slice(&0x8180u16.to_be_bytes()); // QR=1 RD=1 RA=1 NOERROR
        b.extend_from_slice(&1u16.to_be_bytes()); // qd
        b.extend_from_slice(&2u16.to_be_bytes()); // an
        b.extend_from_slice(&0u16.to_be_bytes()); // ns
        b.extend_from_slice(&0u16.to_be_bytes()); // ar
        // question: www.example.com A IN
        let qname_off = b.len();
        b.extend_from_slice(&[3, b'w', b'w', b'w']);
        b.extend_from_slice(&[7, b'e', b'x', b'a', b'm', b'p', b'l', b'e']);
        b.extend_from_slice(&[3, b'c', b'o', b'm']);
        b.push(0);
        b.extend_from_slice(&TYPE_A.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        // answer1: CNAME -> example.com (pointer to qname+4)
        b.extend_from_slice(&[0xC0, (qname_off + 4) as u8]);
        b.extend_from_slice(&TYPE_CNAME.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&300u32.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes()); // RDLEN 占位，稍后回填
        let rd = b.len();
        b.extend_from_slice(&[7, b'e', b'x', b'a', b'm', b'p', b'l', b'e']);
        b.extend_from_slice(&[3, b'c', b'o', b'm']);
        b.push(0);
        let rdlen = (b.len() - rd) as u16;
        b[rd - 2..rd].copy_from_slice(&rdlen.to_be_bytes());
        // answer2: A 93.184.216.34（名字用指针指回 answer1 的名字）
        let name_off = qname_off + 4;
        b.extend_from_slice(&[0xC0, name_off as u8]);
        b.extend_from_slice(&TYPE_A.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&60u32.to_be_bytes());
        b.extend_from_slice(&4u16.to_be_bytes());
        b.extend_from_slice(&[93, 184, 216, 34]);
        b
    }

    #[test]
    fn parse_response_with_compression() {
        let raw = build_response_with_pointer();
        let msg = parse_message(&raw).unwrap();
        assert_eq!(msg.id, 0xABCD);
        assert!(msg.is_noerror());
        assert!(!msg.truncated);
        assert_eq!(msg.questions.len(), 1);
        assert_eq!(msg.questions[0].name, "www.example.com");
        assert_eq!(msg.answers.len(), 2);
        assert_eq!(msg.cname_chain(), vec!["example.com".to_string()]);
        assert_eq!(
            msg.ip_answers(),
            vec!["93.184.216.34".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(msg.min_ttl(), Some(60));
    }

    #[test]
    fn parse_rejects_short_buffer() {
        assert!(parse_message(&[0u8; 5]).is_err());
    }

    #[test]
    fn parse_rejects_bad_pointer_loop() {
        // 指针指向自己 → 必须被检测为畸形
        let mut b = vec![0u8; 12];
        b[4..6].copy_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&[0xC0, 12]);
        b.extend_from_slice(&TYPE_A.to_be_bytes());
        b.extend_from_slice(&1u16.to_be_bytes());
        assert!(parse_message(&b).is_err());
    }

    #[test]
    fn type_name_and_parse_type_roundtrip() {
        for t in [TYPE_A, TYPE_AAAA, TYPE_CNAME, TYPE_TXT, TYPE_HTTPS] {
            let n = type_name(t);
            assert_eq!(parse_type(n).unwrap(), t);
        }
        assert!(parse_type("NOPE").is_err());
    }

    #[test]
    fn nxdomain_detection() {
        let mut b = vec![0u8; 12];
        b[2..4].copy_from_slice(&0x8183u16.to_be_bytes()); // rcode=3
        let m = parse_message(&b).unwrap();
        assert!(m.is_nxdomain());
        assert_eq!(m.rcode_name(), "NXDOMAIN");
    }
}
