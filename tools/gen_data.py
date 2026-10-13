#!/usr/bin/env python3
"""生成 NoFakeIP 内置数据文件。

数据来源（均为公开数据）：
- 国内域名：felixonmars/dnsmasq-china-list  accelerated-domains.china.conf
- 国内 IP：17mon/china_ip_list
- 国外域名：Majestic Million 榜单（剔除国内域名后取头部）

产出：
- data/china_domains.txt
- data/foreign_domains.txt
- data/china_ip_cidr.txt
"""

import csv
import os
import re
import sys

SRC = r"D:/Library/DevelopmentKit/downloads/ipdata"
OUT = r"D:/Develop/Ai/NoFakeIP/crates/nofakeip-core/data"

# 常见多级公共后缀（用于把域名归约到「可注册域名」）
MULTI_SUFFIX = {
    "com.cn", "net.cn", "org.cn", "gov.cn", "edu.cn", "ac.cn", "mil.cn",
    "com.hk", "org.hk", "edu.hk", "gov.hk", "net.hk", "idv.hk",
    "com.tw", "org.tw", "edu.tw", "gov.tw", "net.tw", "idv.tw",
    "co.jp", "or.jp", "ne.jp", "ac.jp", "go.jp", "ad.jp", "ed.jp", "gr.jp", "lg.jp",
    "co.kr", "or.kr", "ne.kr", "re.kr", "pe.kr", "go.kr", "ac.kr",
    "co.uk", "org.uk", "me.uk", "ac.uk", "gov.uk", "net.uk", "sch.uk",
    "com.au", "net.au", "org.au", "edu.au", "gov.au", "id.au", "asn.au",
    "com.sg", "edu.sg", "gov.sg", "org.sg", "net.sg", "per.sg",
    "com.my", "net.my", "org.my", "edu.my", "gov.my",
    "co.in", "net.in", "org.in", "gen.in", "firm.in", "ind.in", "ac.in", "gov.in", "edu.in",
    "com.br", "net.br", "org.br", "gov.br", "edu.br",
    "com.ru", "net.ru", "org.ru", "msk.ru", "spb.ru",
    "com.mx", "net.mx", "org.mx", "edu.mx", "gob.mx",
    "com.tr", "net.tr", "org.tr", "edu.tr", "gov.tr",
    "com.vn", "net.vn", "org.vn", "edu.vn", "gov.vn",
    "co.th", "or.th", "in.th", "go.th", "ac.th", "mi.th", "net.th",
    "com.ph", "net.ph", "org.ph", "edu.ph", "gov.ph",
    "co.id", "or.id", "ac.id", "go.id", "web.id", "sch.id", "my.id",
    "com.ar", "net.ar", "org.ar", "gov.ar", "edu.ar",
    "co.za", "org.za", "net.za", "gov.za", "ac.za",
    "com.sa", "net.sa", "org.sa", "gov.sa", "edu.sa",
    "com.ua", "net.ua", "org.ua", "gov.ua", "edu.ua",
    "co.nz", "net.nz", "org.nz", "govt.nz", "ac.nz",
    "com.pl", "net.pl", "org.pl", "edu.pl", "gov.pl",
    "com.es", "nom.es", "org.es", "gob.es", "edu.es",
    "com.eg", "net.eg", "org.eg", "edu.eg", "gov.eg",
    "com.pk", "net.pk", "org.pk", "edu.pk", "gov.pk",
    "com.bd", "net.bd", "org.bd", "edu.bd", "gov.bd",
    "com.ng", "net.ng", "org.ng", "edu.ng", "gov.ng",
    "co.il", "org.il", "net.il", "ac.il", "gov.il",
    "com.co", "net.co", "org.co", "edu.co", "gov.co",
    "com.pe", "net.pe", "org.pe", "edu.pe", "gob.pe",
    "com.cl", "net.cl", "org.cl", "edu.cl", "gob.cl",
    "com.ve", "net.ve", "org.ve", "edu.ve", "gob.ve",
}

DOMAIN_RE = re.compile(r"^[a-z0-9]([a-z0-9\-_]*[a-z0-9])?(\.[a-z0-9]([a-z0-9\-_]*[a-z0-9])?)+$")


def registrable(domain: str) -> str:
    """把域名归约到可注册域名（example.a.b.com -> b.com）。"""
    d = domain.strip().lower().rstrip(".")
    if not d:
        return ""
    parts = d.split(".")
    if len(parts) < 2:
        return d
    # 找到最长的多级后缀
    best = None
    for i in range(1, min(len(parts), 4)):
        cand = ".".join(parts[-i:])
        if cand in MULTI_SUFFIX:
            best = cand
    if best is not None:
        keep = best.count(".") + 2
        return ".".join(parts[-keep:])
    return ".".join(parts[-2:])


def valid(d: str) -> bool:
    if not d or len(d) > 253:
        return False
    if "." not in d:
        return False
    if d.startswith("-") or d.endswith("-"):
        return False
    if "_" in d:
        return False
    # 排除纯 IP 与明显的非法项
    if re.fullmatch(r"[0-9.]+", d):
        return False
    return bool(DOMAIN_RE.match(d))


def load_cn_domains():
    path = os.path.join(SRC, "accelerated-domains.china.conf")
    out = set()
    with open(path, "r", encoding="utf-8", errors="ignore") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            m = re.match(r"server=/([^/]+)/", line)
            if not m:
                continue
            r = registrable(m.group(1))
            if valid(r):
                out.add(r)
    return out


def load_foreign_domains(cn_set, limit=20000):
    path = os.path.join(SRC, "majestic_million.csv")
    out = []
    seen = set()
    with open(path, "r", encoding="utf-8", errors="ignore", newline="") as f:
        reader = csv.reader(f)
        next(reader, None)  # header
        for row in reader:
            if len(row) < 3:
                continue
            r = registrable(row[2])
            if not valid(r) or r in seen or r in cn_set:
                continue
            seen.add(r)
            out.append(r)
            if len(out) >= limit:
                break
    return out


def load_cn_cidr():
    path = os.path.join(SRC, "china_ip_list.txt")
    out = []
    with open(path, "r", encoding="utf-8", errors="ignore") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if "/" in line and re.fullmatch(r"[0-9.]+/\d{1,2}", line):
                out.append(line)
    return out


def write_txt(path, header, items):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(header)
        for it in items:
            f.write(it + "\n")
    print(f"  -> {os.path.basename(path)}: {len(items)} 条, {os.path.getsize(path)/1024:.1f} KB")


def main():
    print("加载国内域名 ...")
    cn = load_cn_domains()
    print(f"  国内域名（归约去重后）: {len(cn)}")

    print("加载国外域名（Majestic Million，剔除国内）...")
    foreign = load_foreign_domains(cn)
    print(f"  国外域名: {len(foreign)}")

    print("加载国内 IP 段 ...")
    cidr = load_cn_cidr()
    print(f"  国内 CIDR: {len(cidr)}")

    print("写入 ...")
    write_txt(
        os.path.join(OUT, "china_domains.txt"),
        "# NoFakeIP 国内域名规则库\n"
        "# 来源: felixonmars/dnsmasq-china-list (accelerated-domains.china.conf)，已归约为可注册域名\n"
        "# 格式: 每行一个可注册域名；匹配该域名及其所有子域\n"
        "# 注意: 本文件由 tools/gen_data.py 生成，手工修改会在下次生成时被覆盖。\n"
        "#       需要用户级覆盖请放到用户数据目录的 data/china_domains.txt。\n\n",
        sorted(cn),
    )
    write_txt(
        os.path.join(OUT, "foreign_domains.txt"),
        "# NoFakeIP 国外域名规则库\n"
        "# 来源: Majestic Million 榜单（已剔除国内域名）\n"
        "# 格式: 每行一个可注册域名；匹配该域名及其所有子域\n"
        "# 注意: 本文件由 tools/gen_data.py 生成，手工修改会在下次生成时被覆盖。\n\n",
        foreign,
    )
    write_txt(
        os.path.join(OUT, "china_ip_cidr.txt"),
        "# NoFakeIP 中国 IP 网段（离线归属地判定用）\n"
        "# 来源: 17mon/china_ip_list\n"
        "# 用途: 判断某个 IP 是否属于中国大陆主要网段（近似，非完整 GeoIP）\n\n",
        cidr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
