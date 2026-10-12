package main

import (
	"os"
	"path/filepath"
	"testing"

	"cerlesse/tools/internal/orchestrator"
)

// 场景分组（纯内核化阶段五）：内核侧 `-exclude-tag userland`、
// 跨仓库侧 `-tag userland`。这两个方向都要有测试，否则「少跑一个场景」是静默的。

func sample() []orchestrator.Scenario {
	return []orchestrator.Scenario{
		{Name: "boot"},
		{Name: "fs"},
		{Name: "user", Tags: []string{"userland"}},
		{Name: "programs", Tags: []string{"userland"}},
	}
}

func names(scs []orchestrator.Scenario) []string {
	out := make([]string, 0, len(scs))
	for _, sc := range scs {
		out = append(out, sc.Name)
	}
	return out
}

func TestFilterByTagKeepAndDrop(t *testing.T) {
	// keep=true：只保留带标签的（这就是 `-tag userland`）
	got := names(filterByTag(sample(), []string{"userland"}, true))
	want := []string{"user", "programs"}
	if len(got) != len(want) {
		t.Fatalf("keep: got %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("keep: got %v, want %v", got, want)
		}
	}

	// keep=false：跳过带标签的（这就是内核侧 `-exclude-tag userland`）
	got = names(filterByTag(sample(), []string{"userland"}, false))
	want = []string{"boot", "fs"}
	if len(got) != len(want) {
		t.Fatalf("drop: got %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("drop: got %v, want %v", got, want)
		}
	}
}

func TestFilterByTagUnknownTagSelectsNothing(t *testing.T) {
	if got := filterByTag(sample(), []string{"nosuchtag"}, true); len(got) != 0 {
		t.Fatalf("未知标签应选中 0 个场景，得到 %v", names(got))
	}
	if got := filterByTag(sample(), []string{"nosuchtag"}, false); len(got) != len(sample()) {
		t.Fatalf("未知标签不应排除任何场景，得到 %v", names(got))
	}
}

// 拼错的标签必须在**配置加载**时报错，而不是静默少跑：词表在 KnownTags。
func TestLoadConfigRejectsUnknownTag(t *testing.T) {
	cfg := `{"concurrency":1,"scenarios":[
		{"name":"boot","cmd":["true"],"serialLog":"build/x.log"},
		{"name":"user","cmd":["true"],"serialLog":"build/y.log","tags":["user-land"]}
	]}`
	path := filepath.Join(t.TempDir(), "orchestrate.json")
	if err := os.WriteFile(path, []byte(cfg), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := orchestrator.LoadConfig(path); err == nil {
		t.Fatal("未知标签 `user-land` 必须让 LoadConfig 失败")
	}
}

// 词表里的标签 + 合法的场景配置必须能加载（正例）。
func TestLoadConfigAcceptsKnownTag(t *testing.T) {
	cfg := `{"concurrency":1,"scenarios":[
		{"name":"user","cmd":["true"],"serialLog":"build/y.log","tags":["userland"]}
	]}`
	path := filepath.Join(t.TempDir(), "orchestrate.json")
	if err := os.WriteFile(path, []byte(cfg), 0o644); err != nil {
		t.Fatal(err)
	}
	loaded, err := orchestrator.LoadConfig(path)
	if err != nil {
		t.Fatalf("合法标签应能加载：%v", err)
	}
	if len(loaded.Scenarios) != 1 || len(loaded.Scenarios[0].Tags) != 1 {
		t.Fatalf("标签没有解析出来：%+v", loaded.Scenarios)
	}
}
