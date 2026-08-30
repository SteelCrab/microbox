# micro-gui v0.1 아키텍처

상태: Draft  
대상: 첫 번째 동작 가능한 Linux 프로토타입

## 1. 성공 기준

v0.1의 완료 조건은 Kitty Graphics 호환 터미널에서 `micro-gui run xeyes`를
실행한 뒤 다음 작업을 할 수 있는 것입니다.

1. 전용 X11 디스플레이에서 애플리케이션을 시작한다.
2. 창의 프레임을 터미널에 지속적으로 표시한다.
3. 키보드, 클릭, 포인터 이동을 애플리케이션에 전달한다.
4. 터미널 크기 변경에 맞춰 표시 영역과 입력 좌표를 갱신한다.
5. 애플리케이션 종료, `Ctrl-C`, 오류 발생 시 자식 프로세스와 임시 자원을
   정리한다.

v0.1은 데스크톱 환경, 범용 창 관리자 또는 원격 데스크톱 서버를 만들지
않습니다.

## 2. 범위

### 포함

- Linux 호스트
- 단일 포그라운드 세션과 단일 최상위 애플리케이션
- X11/Xvfb 디스플레이 백엔드
- Kitty Graphics Protocol의 direct RGB 전송
- 전체 프레임과 dirty-tile 판별
- Kitty keyboard protocol을 우선 사용하고 일반 터미널 키 입력을 fallback으로 사용
- SGR mouse event 처리
- Native runtime

### 제외

- Wayland/Weston 백엔드
- OCI 이미지 pull 및 rootfs 조립
- 데몬, 영구 세션, `ps`와 `stop`
- 오디오, 클립보드, drag-and-drop, IME
- 다중 창 합성 및 창 장식
- Firecrab 실행 백엔드
- macOS와 Windows 호스트

제외 항목은 인터페이스 경계를 고려하되 v0.1 코드 경로에는 넣지 않습니다.

## 3. 주요 결정

### ADR-001: X11/Xvfb를 첫 디스플레이 백엔드로 사용

X11은 화면 읽기와 합성 입력을 위한 프로토콜이 성숙해 있고, `xeyes`처럼
작은 검증용 애플리케이션이 풍부합니다. v0.1은 Xvfb에 애플리케이션 크기와
동일한 root window를 만들고 별도 데스크톱 환경 없이 애플리케이션을 실행합니다.

최상위 창의 위치와 크기를 강제하는 최소 정책은 micro-gui가 담당합니다.
범용 window manager는 실행하지 않습니다. Wayland는 캡처와 입력 주입 경계가
백엔드마다 달라 v0.1 이후 별도 backend로 추가합니다.

### ADR-002: Native는 호스트 프로세스이며 보안 격리가 아님

Native backend는 전용 X display와 프로세스 그룹을 제공하지만 파일 시스템,
네트워크, 커널을 격리하지 않습니다. Alpine 기반 rootfs/OCI 실행은 별도 runtime
backend로 구현합니다. 강한 격리가 필요한 경우에만 향후 Firecrab backend를
선택합니다.

이 구분은 문서와 CLI 진단 출력에 명시해 native 실행을 sandbox로 오해하지
않게 합니다.

### ADR-003: 첫 프레임은 RGB24 전체 전송

첫 프레임은 Kitty Graphics direct transmission(`f=24`)으로 전송합니다. payload는
base64 인코딩 후 4096-byte command chunk로 나눕니다. v0.1 최적화 순서는 다음과
같습니다.

1. 전체 프레임으로 정확성과 lifecycle 검증
2. 고정 크기 tile의 변경 여부 계산
3. 변경 면적이 임계값보다 작으면 tile만 갱신
4. 변경 면적이 크거나 크기가 바뀌면 전체 프레임 갱신

PNG 인코딩과 shared-memory 전송은 측정 결과가 필요할 때 추가합니다. 초기
구현의 목표는 낮은 지연과 단순한 오류 경로입니다.

### ADR-004: v0.1 session은 프로세스에 종속

`micro-gui run` 프로세스가 session owner입니다. 클라이언트가 종료되면 GUI
애플리케이션과 Xvfb도 종료합니다. 백그라운드 session registry가 없으므로
v0.1에는 `ps`와 `stop`을 노출하지 않습니다.

## 4. 구성 요소

```text
┌──────────────── Terminal frontend ────────────────┐
│ capability probe │ keyboard/mouse │ Kitty encoder │
└──────────────────────────┬─────────────────────────┘
                           │ InputEvent / Frame
┌──────────────── Session controller ───────────────┐
│ state machine │ cancellation │ cleanup ownership  │
└──────────────┬───────────────────────┬─────────────┘
               │                       │
┌──── Runtime backend ────┐  ┌──── Display backend ────┐
│ app process group       │  │ Xvfb lifecycle          │
│ environment and exit    │  │ frame capture / XTest   │
└─────────────────────────┘  └──────────────────────────┘
```

### Terminal frontend

- stdout이 TTY인지 검사합니다.
- capability query를 보내기 전에 명시적인 timeout을 둡니다.
- alternate screen, raw mode, mouse reporting을 하나의 guard가 소유합니다.
- guard가 drop되면 cursor와 terminal mode를 원복합니다.
- frame write 중에는 단일 writer만 stdout을 소유합니다.

### Display backend

예정된 내부 인터페이스는 다음 책임만 가집니다.

```rust,ignore
trait DisplayBackend {
    fn size(&self) -> (u32, u32);
    fn capture(&mut self) -> Result<Frame, DisplayError>;
    fn inject(&mut self, event: InputEvent) -> Result<(), DisplayError>;
    fn resize(&mut self, width: u32, height: u32) -> Result<(), DisplayError>;
}
```

X11 구현은 우선 `GetImage`로 정확성을 확인한 뒤 MIT-SHM을 선택적 fast path로
추가합니다. 입력은 XTEST extension을 사용합니다. extension이 없으면 session
시작 단계에서 지원되지 않는 기능을 명확히 보고합니다.

### Session controller

상태 전이는 다음과 같습니다.

```text
Created → Starting → Running → Stopping → Exited
              └────────┴────────┴────────→ Failed
```

terminal guard, Xvfb, application process는 생성의 역순으로 정리합니다. 일부
정리가 실패해도 나머지 정리를 계속하고 최초 오류와 cleanup 오류를 함께
기록합니다.

## 5. 실행 흐름

```text
parse CLI
  → probe terminal
  → allocate private DISPLAY
  → start Xvfb
  → wait until X11 is ready (bounded timeout)
  → start application in its own process group
  → discover/map top-level window
  → enter terminal mode
  → render/input event loop
  → observe app exit or cancellation
  → restore terminal and terminate owned processes
```

event loop에는 세 종류의 작업이 들어옵니다.

- Frame tick: 새 frame 캡처, 이전 frame과 비교, 변경 영역 전송
- Terminal input: escape sequence decode, 좌표 변환, display backend에 주입
- Lifecycle event: resize, signal, application exit, backend error

renderer가 terminal 처리 속도보다 빠를 경우 대기 중인 frame을 누적하지 않고
가장 최신 frame 하나만 유지합니다. 입력 event는 순서를 유지합니다.

## 6. 좌표와 크기

터미널 mouse event는 cell 좌표이고 GUI는 pixel 좌표를 사용합니다. v0.1은
표시된 image rectangle을 기준으로 선형 변환하고 범위를 벗어난 입력을
clamp합니다. letterbox가 생기면 여백의 event는 GUI에 전달하지 않습니다.

terminal resize 시에는 다음 순서를 사용합니다.

1. 새 cell/pixel 크기를 읽습니다.
2. image placement를 계산합니다.
3. X display와 application window를 resize합니다.
4. 이전 frame cache를 폐기하고 전체 frame을 전송합니다.

## 7. 오류와 진단

사용자에게는 단계와 해결 가능한 원인을 함께 제공합니다.

- terminal graphics capability를 확인할 수 없음
- Xvfb 실행 파일이 없음
- X display readiness timeout
- 애플리케이션을 찾을 수 없거나 즉시 종료됨
- XTEST 또는 capture extension 미지원
- stdout write 실패 또는 terminal 연결 종료

`micro-gui doctor`는 상태를 변경하지 않고 TTY와 알려진 terminal 환경을
진단합니다. 환경 변수 검사는 힌트이며 최종 capability 판정은 protocol query로
대체합니다.

## 8. 보안 경계

- terminal escape sequence에 들어가는 숫자는 범위를 검사합니다.
- 애플리케이션 command는 shell string으로 조합하지 않고 argv로 전달합니다.
- private X authority를 사용하고 다른 local client의 접속을 허용하지 않습니다.
- 크기와 frame buffer allocation에 상한을 둡니다.
- Native backend는 sandbox로 표시하지 않습니다.
- Firecrab API는 향후 별도 crate/feature 경계에서 연결합니다.

## 9. 현재 구현 상태

현재 저장소에는 다음 기반이 구현되어 있습니다.

- `doctor` 환경 진단
- RGB24 frame 모델과 buffer 검증
- Kitty Graphics base64/chunk encoder
- 생성된 RGB frame을 출력하는 `demo`
- tile 단위 frame 변경 판별
- terminal cell에서 frame pixel로의 좌표 변환
- session 상태 전이 검증

X11 display backend와 terminal input decoder가 연결되기 전까지 `run`은
애플리케이션을 시작하지 않고 명시적인 오류를 반환합니다.
