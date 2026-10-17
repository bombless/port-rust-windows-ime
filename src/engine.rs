use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub text: String,
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Backspace,
    Escape,
    Up,
    Down,
    Space,
    Enter,
    Digit(u8),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    PassThrough,
    Update(String),
    Commit(String),
    Cancel,
    Consumed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub composition: String,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    pub visible: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Engine {
    composition: String,
    candidates: Vec<Candidate>,
    selected: usize,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            composition: self.composition.clone(),
            candidates: self.candidates.clone(),
            selected: self.selected,
            visible: !self.composition.is_empty() && !self.candidates.is_empty(),
        }
    }

    /// Predict key consumption without mutating state (required by TSF's test phase).
    pub fn test_key(&self, key: Key) -> bool {
        if self.composition.is_empty() {
            return matches!(key, Key::Character(c) if c.is_ascii_alphabetic());
        }
        match key {
            Key::Character(c) => c.is_ascii_alphabetic(),
            Key::Backspace | Key::Escape | Key::Space | Key::Enter => true,
            Key::Up | Key::Down => !self.candidates.is_empty(),
            Key::Digit(n @ 1..=9) => usize::from(n - 1) < self.candidates.len(),
            Key::Digit(_) | Key::Other => false,
        }
    }

    pub fn handle_key(&mut self, key: Key) -> KeyOutcome {
        if self.composition.is_empty() {
            if let Key::Character(c) = key {
                if c.is_ascii_alphabetic() {
                    self.composition.push(c.to_ascii_lowercase());
                    self.refresh();
                    return KeyOutcome::Update(self.composition.clone());
                }
            }
            return KeyOutcome::PassThrough;
        }
        match key {
            Key::Character(c) if c.is_ascii_alphabetic() => {
                self.composition.push(c.to_ascii_lowercase());
                self.refresh();
                KeyOutcome::Update(self.composition.clone())
            }
            Key::Backspace => {
                self.composition.pop();
                if self.composition.is_empty() {
                    self.clear();
                    KeyOutcome::Cancel
                } else {
                    self.refresh();
                    KeyOutcome::Update(self.composition.clone())
                }
            }
            Key::Escape => {
                self.clear();
                KeyOutcome::Cancel
            }
            Key::Up if !self.candidates.is_empty() => {
                self.selected = self.selected.saturating_sub(1);
                KeyOutcome::Consumed
            }
            Key::Down if !self.candidates.is_empty() => {
                self.selected = (self.selected + 1).min(self.candidates.len() - 1);
                KeyOutcome::Consumed
            }
            Key::Digit(n @ 1..=9) => {
                let i = usize::from(n - 1);
                if i < self.candidates.len() {
                    self.commit(i)
                } else {
                    KeyOutcome::PassThrough
                }
            }
            Key::Space | Key::Enter => {
                if self.candidates.is_empty() {
                    let raw = self.composition.clone();
                    self.clear();
                    KeyOutcome::Commit(raw)
                } else {
                    self.commit(self.selected)
                }
            }
            _ => KeyOutcome::PassThrough,
        }
    }

    fn commit(&mut self, index: usize) -> KeyOutcome {
        if let Some(c) = self.candidates.get(index) {
            let text = c.text.clone();
            self.clear();
            KeyOutcome::Commit(text)
        } else {
            KeyOutcome::PassThrough
        }
    }
    fn clear(&mut self) {
        self.composition.clear();
        self.candidates.clear();
        self.selected = 0;
    }
    fn refresh(&mut self) {
        self.candidates = lookup(&self.composition);
        self.selected = 0;
    }
}

/// Seed entries are kept first to preserve useful candidate ordering.
const SEED_WORDS: &[(&str, &str)] = &[
    ("ni", "你"),
    ("ni", "呢"),
    ("ni", "尼"),
    ("ni", "泥"),
    ("ni", "拟"),
    ("hao", "好"),
    ("hao", "号"),
    ("hao", "毫"),
    ("hao", "浩"),
    ("hao", "豪"),
    ("nihao", "你好"),
    ("nihao", "您好"),
    ("wo", "我"),
    ("wo", "握"),
    ("women", "我们"),
    ("women", "我们的"),
    ("ta", "他"),
    ("ta", "她"),
    ("ta", "它"),
    ("tamen", "他们"),
    ("tamen", "她们"),
    ("shi", "是"),
    ("shi", "时"),
    ("shi", "事"),
    ("shi", "使"),
    ("shi", "世"),
    ("shijie", "世界"),
    ("shijie", "世界上"),
    ("zhong", "中"),
    ("zhong", "种"),
    ("zhongguo", "中国"),
    ("zhongguoren", "中国人"),
    ("guo", "国"),
    ("guo", "过"),
    ("ren", "人"),
    ("renmin", "人民"),
    ("renmin", "人民群众"),
    ("min", "民"),
    ("de", "的"),
    ("de", "得"),
    ("de", "地"),
    ("yi", "一"),
    ("yi", "以"),
    ("shi", "是"),
    ("bu", "不"),
    ("bu", "部"),
    ("le", "了"),
    ("le", "乐"),
    ("zai", "在"),
    ("zai", "再"),
    ("you", "有"),
    ("you", "又"),
    ("you", "由"),
    ("ma", "吗"),
    ("ma", "嘛"),
    ("ma", "马"),
    ("ma", "妈"),
    ("me", "么"),
    ("什么", "什么"),
    ("shenme", "什么"),
    ("zenme", "怎么"),
    ("zenmeyang", "怎么样"),
    ("keyi", "可以"),
    ("keyi", "可以的"),
    ("buyao", "不要"),
    ("bukeyi", "不可以"),
    ("xiexie", "谢谢"),
    ("duibuqi", "对不起"),
    ("meiguanxi", "没关系"),
    ("zaijian", "再见"),
    ("zaijian", "再见了"),
    ("qing", "请"),
    ("qingwen", "请问"),
    ("qingchu", "清楚"),
    ("mingbai", "明白"),
    ("zhidao", "知道"),
    ("juede", "觉得"),
    ("xihuan", "喜欢"),
    ("gaoxing", "高兴"),
    ("kaixin", "开心"),
    ("pengyou", "朋友"),
    ("jia", "家"),
    ("jiaren", "家人"),
    ("gongzuo", "工作"),
    ("xuexi", "学习"),
    ("xuexiao", "学校"),
    ("laoshi", "老师"),
    ("xuesheng", "学生"),
    ("tongxue", "同学"),
    ("dianhua", "电话"),
    ("shouji", "手机"),
    ("diannao", "电脑"),
    ("wangluo", "网络"),
    ("shijian", "时间"),
    ("jintian", "今天"),
    ("mingtian", "明天"),
    ("zuotian", "昨天"),
    ("xianzai", "现在"),
    ("yihou", "以后"),
    ("yijing", "已经"),
    ("keyi", "可以"),
    ("yinwei", "因为"),
    ("suoyi", "所以"),
    ("danshi", "但是"),
    ("ruguo", "如果"),
    ("haishi", "还是"),
    ("huozhe", "或者"),
    ("ranhou", "然后"),
    ("zheyang", "这样"),
    ("nayang", "那样"),
    ("zheli", "这里"),
    ("nali", "那里"),
    ("nali", "哪里"),
    ("shanghai", "上海"),
    ("beijing", "北京"),
    ("guangzhou", "广州"),
    ("shenzhen", "深圳"),
    ("zhongwen", "中文"),
    ("hanyu", "汉语"),
    ("putonghua", "普通话"),
    ("pinyin", "拼音"),
    ("shuru", "输入"),
    ("shurufa", "输入法"),
    ("ciku", "词库"),
    ("ci", "词"),
    ("yuyan", "语言"),
    ("wenzi", "文字"),
    ("hanzi", "汉字"),
    ("danci", "单词"),
    ("chengxu", "程序"),
    ("ruanjian", "软件"),
    ("yingjian", "硬件"),
    ("xitong", "系统"),
    ("shuju", "数据"),
    ("wenjian", "文件"),
    ("wangzhan", "网站"),
    ("kaiyuan", "开源"),
    ("mianfei", "免费"),
    ("anquan", "安全"),
    ("yinsi", "隐私"),
    ("shijian", "实践"),
    ("xianzhi", "限制"),
    ("wenti", "问题"),
    ("jiejue", "解决"),
    ("fangfa", "方法"),
    ("xuyao", "需要"),
    ("xiangyao", "想要"),
    ("xiwang", "希望"),
    ("yinggai", "应该"),
    ("neng", "能"),
    ("nengli", "能力"),
    ("haode", "好的"),
    ("meiyou", "没有"),
    ("keyi", "可以"),
    ("haochi", "好吃"),
    ("haokan", "好看"),
    ("haoting", "好听"),
    ("daxue", "大学"),
    ("gongsi", "公司"),
    ("shijian", "事件"),
    ("shangban", "上班"),
    ("xiaban", "下班"),
    ("chifan", "吃饭"),
    ("shuijiao", "睡觉"),
    ("yundong", "运动"),
    ("yisheng", "医生"),
    ("yiyuan", "医院"),
    ("jiankang", "健康"),
    ("shenghuo", "生活"),
    ("meitian", "每天"),
    ("dajia", "大家"),
    ("ziji", "自己"),
    ("bijiao", "比较"),
    ("zuihou", "最后"),
    ("zuijin", "最近"),
    ("zhuyao", "主要"),
    ("zhengque", "正确"),
    ("zhongyao", "重要"),
    ("tongguo", "通过"),
    ("kaishi", "开始"),
    ("jieshu", "结束"),
    ("shiyong", "使用"),
    ("gongneng", "功能"),
    ("tishi", "提示"),
    ("bangzhu", "帮助"),
    ("wanshang", "晚上"),
    ("zaoshang", "早上"),
    ("zhongwu", "中午"),
    ("xianzai", "现在"),
];

#[derive(Debug, Clone)]
struct RimeEntry {
    code: String,
    word: String,
    // Rime's optional third column is a percentage weight for this word/reading.
    // Store hundredths of a percentage point to avoid floating-point ordering.
    weight: u32,
}

static RIME_ENTRIES: std::sync::OnceLock<Vec<RimeEntry>> = std::sync::OnceLock::new();

fn rime_entries() -> &'static [RimeEntry] {
    RIME_ENTRIES.get_or_init(|| {
        let mut in_data = false;
        let mut entries = Vec::new();
        for line in include_str!("../data/luna_pinyin.dict.yaml").lines() {
            if !in_data {
                if line.trim() == "..." {
                    in_data = true;
                }
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split('\t');
            let Some(word) = fields.next() else { continue };
            let Some(raw_code) = fields.next() else {
                continue;
            };
            let weight = fields
                .next()
                .and_then(|raw| raw.trim().strip_suffix('%'))
                .and_then(|raw| raw.parse::<f64>().ok())
                .map(|value| (value * 100.0).round() as u32)
                .unwrap_or(0);
            let code: String = raw_code
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .map(|c| c.to_ascii_lowercase())
                .collect();
            if !code.is_empty() && !word.is_empty() {
                entries.push(RimeEntry {
                    code,
                    word: word.to_owned(),
                    weight,
                });
            }
        }
        entries
    })
}

/// Search curated seed entries first, then Rime entries ranked by their
/// upstream percentage weights. Unweighted entries retain dictionary order
/// relative to other unweighted entries. Spaces in pinyin are removed to
/// match the engine's unspaced composition.
fn lookup(code: &str) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = Vec::new();
    for (pinyin, word) in SEED_WORDS {
        if (*pinyin == code || pinyin.starts_with(code))
            && !candidates.iter().any(|candidate| candidate.text == *word)
        {
            candidates.push(Candidate {
                text: (*word).to_owned(),
                code: (*pinyin).to_owned(),
            });
        }
    }

    let mut matches: Vec<&RimeEntry> = rime_entries()
        .iter()
        .filter(|entry| entry.code == code || entry.code.starts_with(code))
        .collect();
    // Stable sorting preserves upstream dictionary order for equal weights.
    matches.sort_by(|a, b| b.weight.cmp(&a.weight));

    for entry in matches {
        if candidates.len() >= 100 {
            break;
        }
        if !candidates
            .iter()
            .any(|candidate| candidate.text == entry.word)
        {
            candidates.push(Candidate {
                text: entry.word.clone(),
                code: entry.code.clone(),
            });
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    fn type_text(engine: &mut Engine, text: &str) {
        for c in text.chars() {
            assert!(engine.test_key(Key::Character(c)));
            assert!(matches!(
                engine.handle_key(Key::Character(c)),
                KeyOutcome::Update(_)
            ));
        }
    }
    #[test]
    fn prediction_is_pure() {
        let mut e = Engine::new();
        assert!(e.test_key(Key::Character('n')));
        assert_eq!(e.snapshot().composition, "");
        e.handle_key(Key::Character('n'));
        let before = e.snapshot();
        assert!(e.test_key(Key::Character('i')));
        assert_eq!(e.snapshot(), before);
    }
    #[test]
    fn compose_and_commit_selected_candidate() {
        let mut e = Engine::new();
        type_text(&mut e, "ni");
        assert!(e.snapshot().candidates.iter().any(|c| c.text == "你"));
        assert_eq!(e.handle_key(Key::Space), KeyOutcome::Commit("你".into()));
        assert_eq!(e.snapshot().composition, "");
    }
    #[test]
    fn digit_selects_candidate_by_one_based_index() {
        let mut e = Engine::new();
        type_text(&mut e, "ni");
        assert_eq!(e.handle_key(Key::Digit(2)), KeyOutcome::Commit("呢".into()));
    }
    #[test]
    fn common_phrases_are_available() {
        let mut e = Engine::new();
        type_text(&mut e, "zhongguo");
        assert!(e.snapshot().candidates.iter().any(|c| c.text == "中国"));
        let mut e = Engine::new();
        type_text(&mut e, "xiexie");
        assert!(e.snapshot().candidates.iter().any(|c| c.text == "谢谢"));
    }

    #[test]
    fn loads_entries_from_rime_dictionary_and_normalizes_spaces() {
        let mut e = Engine::new();
        type_text(&mut e, "kafei");
        assert!(e.snapshot().candidates.iter().any(|c| c.text == "咖啡"));
    }

    #[test]
    fn parses_rime_percentage_weights() {
        let mut e = Engine::new();
        type_text(&mut e, "wan");
        // Rime assigns 万's "wan" reading 99.98%, while its "mo" reading
        // has only 0.02%; the "wan" entry must carry the larger parsed weight.
        let entries = rime_entries();
        let wan = entries
            .iter()
            .find(|entry| entry.word == "万" && entry.code == "wan")
            .unwrap();
        let mo = entries
            .iter()
            .find(|entry| entry.word == "万" && entry.code == "mo")
            .unwrap();
        assert_eq!(wan.weight, 9998);
        assert_eq!(mo.weight, 2);
        assert!(e.snapshot().candidates.iter().any(|c| c.text == "万"));
        let candidates = e.snapshot().candidates;
        let index_of = |word: &str| {
            candidates
                .iter()
                .position(|candidate| candidate.text == word)
                .unwrap()
        };
        assert!(
            index_of("皖") < index_of("万"),
            "100% weight should rank above 99.98%"
        );
    }
}
