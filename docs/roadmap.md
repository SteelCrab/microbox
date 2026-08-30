# micro-gui 구현 로드맵

## M0 — Terminal rendering foundation (현재)

- [x] Rust 프로젝트 기본 구조
- [x] TTY/terminal 진단 명령
- [x] Kitty Graphics RGB24 전송과 payload chunking
- [x] 생성 frame 출력 demo
- [x] frame buffer 검증과 dirty-tile 판별
- [x] 입력 좌표 변환 모델
- [x] session 상태 모델

검증:

```sh
cargo test
cargo run -- doctor
cargo run -- demo
```

## M1 — X11 capture PoC

- [x] 사용 가능한 private display 번호 할당
- [x] Xvfb 실행과 readiness timeout
- [x] argv 기반 애플리케이션 실행
- [x] 최상위 window 탐색 및 root 크기에 맞춤
- [x] X11 `GetImage` capture
- [x] 10 FPS 전체 frame 출력
- [x] 종료 시 Xvfb와 application process group 정리

완료 조건: Kitty 호환 터미널에서 `micro-gui run xeyes` 화면이 지속적으로
갱신됩니다. 아직 입력은 필요하지 않습니다.

통합 검증은 system dependency가 필요하므로 기본 test suite와 분리합니다.

```sh
cargo test runtime::native::tests::captures_xeyes_frame -- --ignored
```

## M2 — Interactive session

- [x] terminal raw/alternate-screen guard
- [x] Kitty keyboard event decoder
- [x] 일반 key event fallback
- [x] SGR mouse press/release/move decoder
- [x] XTEST key/button/motion 주입
- [x] `SIGWINCH` 기반 placement/좌표 resize
- [x] `Ctrl-C`, `SIGTERM`, `SIGHUP` 종료 복구

완료 조건: `xeyes`의 시선이 pointer를 따라가고 GTK Demo의 button과 text field를
조작할 수 있습니다.

자동 통합 테스트는 XTEST pointer 이동과 key press/release를 검증합니다. 실제
Kitty 호환 terminal에서의 GTK Demo 수동 조작 확인은 release checklist에 남깁니다.

## M3 — Rendering efficiency

- [x] render loop backpressure
- [x] dirty tile update 연결
- [x] 변경 면적 기반 full/tile 선택
- [x] 목표 frame rate 옵션과 CPU 측정
- [x] MIT-SHM capture fast path

초기 성능 목표:

- idle 상태 CPU 5% 미만(개발 기준 장비에서 측정 조건과 함께 기록)
- 기본 목표 30 FPS, 느린 연결에서는 최신 frame 우선
- 입력 event 순서 보존

측정 조건과 결과는 [performance.md](performance.md)에 기록합니다.

## M4 — v0.1 hardening

- [x] `xeyes`, Xmessage, 사용 가능한 GTK editor smoke test harness
- [x] terminal disconnect와 broken pipe test
- [x] child crash와 Xvfb crash test
- [x] frame allocation 상한과 fuzz 가능한 decoder 분리
- [x] 설치 문서와 demo 절차
- [x] v0.1 release checklist

실제 Kitty/Ghostty/WezTerm 조작, GTK Demo/Leafpad가 설치된 환경의 확인, demo
녹화는 [release checklist](release-checklist.md)의 수동 release gate입니다.

## v0.1 이후

1. [x] Alpine/OCI application image backend (Docker engine, X11 socket,
   disposable lifecycle)
2. [ ] Firecrab MicroVM backend — Firecrab에 bidirectional GUI guest transport가
   먼저 필요
3. Wayland/Weston backend
4. background session daemon과 `ps`/`stop`
5. clipboard, audio, 고급 keyboard/IME
