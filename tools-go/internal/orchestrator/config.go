package orchestrator

import (
	"encoding/json"
	"fmt"
	"os"
)

// KnownTags 是允许出现在场景 `tags` 里的标签词表。
//
// 只有**用户态集成**场景需要标签：它们断言 `/init`、`/bin/*` 或 `libc` 的输出，
// 因此在没有内嵌用户态的内核镜像上必然失败，不能进内核 `make test-all` /
// `make test-orch`（见 docs/pure-kernel.md 阶段五）。内核场景一律不标 ——
// 「默认属于内核」比「每个场景都要记得标」更难搞错。
//
// 词表在这里硬校验，因此拼错标签（例如 `user-land`）会让配置加载失败，
// 而不是静默地少跑一个场景。
var KnownTags = []string{"userland"}

// Config 是编排器的场景配置文件（tests/orchestrate.json）。
type Config struct {
	// Concurrency 是默认并发度（命令行 -parallel 可覆盖）。
	Concurrency int `json:"concurrency"`
	// Scenarios 是待并行执行的场景列表。
	Scenarios []Scenario `json:"scenarios"`
}

// LoadConfig 读取并校验 JSON 配置。
func LoadConfig(path string) (*Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var cfg Config
	if err := json.Unmarshal(data, &cfg); err != nil {
		return nil, fmt.Errorf("parse %s: %w", path, err)
	}
	if err := cfg.Validate(); err != nil {
		return nil, fmt.Errorf("%s: %w", path, err)
	}
	return &cfg, nil
}

// Validate 校验场景约束：名称唯一非空、必须有可执行命令或 serialOnly、必须有日志路径。
func (c *Config) Validate() error {
	if len(c.Scenarios) == 0 {
		return fmt.Errorf("no scenarios")
	}
	seen := make(map[string]bool, len(c.Scenarios))
	for i, sc := range c.Scenarios {
		if sc.Name == "" {
			return fmt.Errorf("scenario #%d: empty name", i)
		}
		if seen[sc.Name] {
			return fmt.Errorf("duplicate scenario name %q", sc.Name)
		}
		seen[sc.Name] = true
		if sc.SerialLog == "" {
			return fmt.Errorf("scenario %q: serialLog is required", sc.Name)
		}
		if !sc.SerialOnly && len(sc.Cmd) == 0 {
			return fmt.Errorf("scenario %q: cmd is required unless serialOnly", sc.Name)
		}
		for _, tag := range sc.Tags {
			if !knownTag(tag) {
				return fmt.Errorf("scenario %q: unknown tag %q (known: %v)", sc.Name, tag, KnownTags)
			}
		}
	}
	return nil
}

// knownTag 报告 tag 是否在 KnownTags 词表里。
func knownTag(tag string) bool {
	for _, t := range KnownTags {
		if t == tag {
			return true
		}
	}
	return false
}
