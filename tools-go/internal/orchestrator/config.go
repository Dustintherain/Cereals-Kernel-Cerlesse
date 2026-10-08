package orchestrator

import (
	"encoding/json"
	"fmt"
	"os"
)

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
	}
	return nil
}
