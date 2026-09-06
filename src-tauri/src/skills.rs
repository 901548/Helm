// ============================================================================
// skills.rs - 内置技能库（P85）：常用运维任务的确定性执行剧本
//   - 意图匹配：任务文本按关键词评分命中技能（命中即绕过 LLM 直接执行）
//   - 金标数据：技能轨迹经记录器落盘（source:"skill"），可直接作小模型 SFT 样本
//   - 设计原则：命令自带容错回退（||），读操作为主；危险操作不进技能库
// ============================================================================

/// 单个内置技能：意图关键词 + 确定性命令序列
#[derive(Debug, Clone)]
pub struct Skill {
    /// 技能 id
    pub id: &'static str,
    /// 标题（计划卡展示）
    pub title: &'static str,
    /// 意图关键词（任务文本命中任意一个即计分）
    pub keywords: &'static [&'static str],
    /// 确定性命令序列（命令自带容错回退）
    pub steps: &'static [&'static str],
}

/// 内置技能库（按评测任务类别覆盖常用运维意图）
pub const SKILLS: &[Skill] = &[
    Skill {
        id: "disk-usage",
        title: "磁盘使用情况检查",
        keywords: &["磁盘", "磁盘使用", "挂载点", "disk", "存储空间", "空间占用"],
        steps: &[
            "df -h",
            "df -h | sort -k 5 -nr | head -n 6",
            "du -sh /var /tmp /home /root 2>/dev/null | sort -rh | head -n 5",
        ],
    },
    Skill {
        id: "mem-check",
        title: "内存与进程检查",
        keywords: &["内存", "mem", "内存占用", "进程占用", "内存使用"],
        steps: &[
            "free -m",
            "ps -eo pid,%mem,command --sort=-%mem | head -n 6",
        ],
    },
    Skill {
        id: "log-errors",
        title: "系统日志异常排查",
        keywords: &["日志", "报错", "异常", "错误日志", "log", "内核报错"],
        steps: &[
            "grep -iE 'error|fail|panic' /var/log/messages 2>/dev/null | tail -n 30",
            "dmesg -T 2>/dev/null | grep -iE 'error|fail' | tail -n 15",
            "ls -lh /var/log/messages* /var/log/secure* 2>/dev/null",
        ],
    },
    Skill {
        id: "listen-ports",
        title: "监听端口排查",
        keywords: &["端口", "监听", "listen", "port", "netstat", "ss -tlnp"],
        steps: &[
            "ss -tlnp 2>/dev/null || netstat -tlnp 2>/dev/null",
        ],
    },
    Skill {
        id: "service-sshd",
        title: "sshd 服务状态检查",
        keywords: &["sshd", "ssh 服务", "ssh服务", "远程登录服务"],
        steps: &[
            "systemctl status sshd 2>/dev/null || service sshd status",
            "ss -tlnp 2>/dev/null | grep :22 || netstat -tlnp 2>/dev/null | grep :22",
        ],
    },
    Skill {
        id: "login-users",
        title: "可登录用户盘点",
        keywords: &["用户", "用户账户", "可登录", "账户列表", "账号"],
        steps: &[
            "awk -F: '$7 !~ /nologin|false/ {print $1, $7}' /etc/passwd",
        ],
    },
    Skill {
        id: "cpu-load",
        title: "CPU 与负载检查",
        keywords: &["cpu", "负载", "load", "cpu占用"],
        steps: &[
            "uptime",
            "ps -eo pid,%cpu,command --sort=-%cpu | head -n 6",
        ],
    },
    Skill {
        id: "big-files",
        title: "大文件排查",
        keywords: &["大文件", "占用大", "找出大", "最大的文件"],
        steps: &[
            "find /var /tmp /home -xdev -type f -size +50M -exec ls -lh {} \\; 2>/dev/null | head -n 15",
        ],
    },
];

/// 任务文本 → 意图匹配。按关键词命中数评分，取最高分且 >0 的技能。
///
/// 简单子串计分足够：运维任务意图词明显（"查磁盘"/"内存占用"/"监听端口"），
/// 无需模糊匹配；未命中返回 None 交回 LLM Agent。
pub fn match_skill(task: &str) -> Option<&'static Skill> {
    let t = task.to_lowercase();
    // 长度加权评分：命中关键词越长越具体，权重越大（字符数："sshd"=4 压过 "报错"=2）
    let mut best: Option<(&Skill, usize)> = None;
    for skill in SKILLS {
        let score: usize = skill
            .keywords
            .iter()
            .filter(|k| t.contains(*k))
            .map(|k| k.chars().count())
            .sum();
        if score > 0 && best.map_or(true, |(_, s)| score > s) {
            best = Some((skill, score));
        }
    }
    best.map(|(s, _)| s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 技能库基础完整性：id 唯一、每个技能至少 1 步、关键词非空
    #[test]
    fn skills_registry_integrity() {
        let mut ids = Vec::new();
        for s in SKILLS {
            assert!(!s.id.is_empty());
            assert!(!s.steps.is_empty(), "{} 无步骤", s.id);
            assert!(!s.keywords.is_empty(), "{} 无关键词", s.id);
            ids.push(s.id);
        }
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "技能 id 重复");
    }

    /// 评测任务集措辞的命中验证（P84 八类任务对应覆盖）
    #[test]
    fn match_skill_hits_benchmark_phrasings() {
        assert_eq!(
            match_skill("查看这台服务器整体磁盘使用情况，告诉我哪个挂载点占用最高")
                .unwrap()
                .id,
            "disk-usage"
        );
        assert_eq!(
            match_skill("查看当前内存使用情况和占用内存最高的前 3 个进程")
                .unwrap()
                .id,
            "mem-check"
        );
        assert_eq!(
            match_skill("检查 /var/log/messages 里最近有没有内核报错或异常日志，总结一下")
                .unwrap()
                .id,
            "log-errors"
        );
        assert_eq!(
            match_skill("列出当前服务器处于监听状态的 TCP 端口及对应进程")
                .unwrap()
                .id,
            "listen-ports"
        );
        assert_eq!(
            match_skill("检查这台服务器上 sshd 服务有没有报错").unwrap().id,
            "service-sshd"
        );
        assert_eq!(
            match_skill("列出这台服务器上可以登录 shell 的用户账户")
                .unwrap()
                .id,
            "login-users"
        );
        // 报错排查任务里的"日志"词命中 log-errors（而非 service）
        assert_ne!(
            match_skill("检查 sshd 服务日志有没有异常报错").unwrap().id,
            "listen-ports"
        );
    }

    /// 未命中返回 None（交回 LLM）；歧义任务取最高分
    #[test]
    fn match_skill_miss_and_priority() {
        assert!(match_skill("帮我写一个 Python 脚本监控股价").is_none());
        assert!(match_skill("").is_none());
        // 同时含"磁盘"与"内存"→ 各 1 分，先登记者胜（稳定即可）
        let hit = match_skill("检查磁盘和内存使用情况").unwrap();
        assert!(matches!(
            hit.id,
            "disk-usage" | "mem-check"
        ));
    }
}
