# Nanami messaging protocol

本書は、A9N MicrokernelのIPC上に構築されているNanamiの通信方式を、IDLとIDL Compilerの設計資料としてまとめたものである。新しいIDLの文法や新しい通信ABIを定義するものではない。

- 調査日: 2026-10-26
- 対象: Nanami 0.6.1、リポジトリrevision `19da5b2`時点の実装
- 対象アーキテクチャ: 現行のx86_64 / AArch64。ともに64-bit little-endian
- 用語: 「現行」はコードで確認した動作、「要件」は互換なbindingを生成するために必要な情報、「検討事項」は現在のABIでは統一されていない点を指す。

## 1. 全体像

Nanamiの通信は、単一の`rpc(method, arguments)`では表現できない。少なくとも次の要素を組み合わせる。

| 要素 | 役割 | データ・権限の所在 |
| --- | --- | --- |
| IPC Port | 接続、制御、短いrequest/reply | Capabilityとmessage registers |
| Shared Memory | ファイル内容、パケット、画像、キュー、構造化payload | 各参加Processの別々のVAに同じbackingをmap |
| Notification Port | キュー更新、readiness、Timer、IRQ、管理処理の起床 | Capabilityのslot-local identifierをOR集約 |
| Alpha | 名前解決、Capabilityの付与、mapping、管理された接続のlifecycle | 各Alpha Domain内の管理状態 |
| サービス固有の状態 | handle、queue cursor、buffer layout、完了判定 | サーバー内、または共有領域 |

典型的な流れは次のとおり。

```text
Client -- AlphaへのCALL --> 名前解決・endpoint付与・共有領域の準備
Client -- 直接CALL ------> Server -- REPLY / REPLY_RECEIVE --> Client
Client <== Shared Memory =================================> Server
Client -- NOTIFY --------> Server     または Server -- NOTIFY --> Client
```

Alphaは通常のサービスrequestを逐次中継しない。接続後のCALLとNOTIFYは、付与済みCapabilityを直接使用する。Shared Memoryの読み書きにも、1アクセスごとのAlpha IPCはない。

次の二つのサービス接続方式が併存する。

1. **既存のPIDベースの接続**: block、VFS、network、input、Honokaなど。相手PIDを使って共有領域やNotification aliasを準備する。
2. **Managed Service**: AlphaがService / Provider / Session / Bindingを分離し、対応profile内で協調的なProvider交換を扱う。既存サービスが自動的にこの方式になるわけではない。

## 2. 型と識別子

### 2.1 Wireの基本単位

`nanami-uapi::Word`は`usize`であり、現在の対象では8 bytesである。通常のIPC payloadはWordの列であって、Rustの構造体をそのまま送信する方式ではない。

- `ServiceRequest`や`ServiceEvent`は、受信済みmessageを表すローカルAPI型である。構造体のmemory layout自体はwire契約ではない。
- message内のbool、enum、bit field、符号付き値、文字列などは、各operationのpacking規約に従う。
- 共有領域には、Word配列、明示的なLE整数、byte列、UTF-8 JSONなどがある。全Shared Memoryに共通のserializerはない。
- `usize`を用いているため、このABIを32-bit / big-endianや異なるWord幅の参加者間へ、そのまま拡張できるとはしない。

### 2.2 数値が同じ型でも意味は異なる

IDLで少なくとも次を区別する必要がある。

| 値 | 意味・scope |
| --- | --- |
| CapabilityDescriptor | 呼出元のCapability space内の探索descriptor。相手のdescriptorとしては使用できない |
| root slot番号 | AlphaがCapabilityを設置する先などを示すローカルslot index。descriptorそのものではない |
| IPC identifier | 使用したCapability aliasに設定されたslot-local値。通常requestの認証・session識別に使用 |
| Notification identifier / badge | NOTIFYに使ったaliasの値。複数回分がORで集約される |
| PID | 対応するAlpha Domain内のProcess番号。別Domainに持ち出せる大域IDではない |
| ServiceId / ProviderId / SessionId | Managed Service用のAlpha-local ID。PIDとは別物 |
| resource handle | file、window、GPU buffer、socketなど。scope・再利用規則・所有者は各サービスが定義 |
| VA | 指定されたProcessのvirtual address。別ProcessのVAと数値が一致する保証はない |
| physical / DMA / MMIO address | 明示的にその権限を持つ低位サービスのためのアドレス。通常の共有buffer参照とは別 |
| offset / length | 指定された共有領域などの内部のbyte位置・長さ。要素数、page数、sector数とも区別する |

Native Processのflat root用descriptor helperは次の値を生成する。

```text
process_slot_descriptor(slot) = 0x0800_0000_0000_0000 | ((slot & 0xff) << 48)
```

これは現在の8-bit root用の配置規約であり、汎用Capability探索や全Domainの固定descriptorを定義するものではない。代表的な初期slotは、self PCB=1、OS Port=2、service endpoint=20、共通Notification=21である。

既存Alphaのdevice slot検査は8..255を対象とするが、占有済み・framework管理slotを自由に再利用してよいわけではない。Managed Serviceの指定slotは23..223で、役割間で重複できない。

## 3. A9N IPCとNanami envelope

### 3.1 Message registers

以下の`MR[n]`はA9Nの仮想message register番号を指す。Rust ABIの`ipc_buffer.configure_message(n, value)`もこの番号を使用する。

| MR | 呼出時の役割 | 受信・結果側の役割 |
| --- | --- | --- |
| 0 | 対象CapabilityDescriptor | Kernel invocationの成否 |
| 1 | Capability operation | KernelのCapabilityError等 |
| 2 | MessageInfo | 受信MessageInfo |
| 3 | identifier用の制御領域 | 使用されたalias由来のidentifier |
| 4以降 | IPC payload | IPC payload |

MR0/1のKernel invocation結果と、MR4のNanami service statusは別レイヤーである。

現行HALはMR0..9をCPU registerで扱い、MR10以降をProcessのIPC bufferで扱う。標準requestはMR4..8、標準replyはMR4..6に収まる。MR9にはRust helperが0を書き込むが、標準requestの追加argumentとして公開してはいない。

KernelのIPC bufferは1 page以内であり、64-bit時はmessage用494 Words、Capability転送元descriptor配列16件、転送先Node / indexを持つ。これはファイルや画像のためのShared Memoryとは別の領域である。

### 3.2 MessageInfo

```text
bit 0       : block
bits 1..8   : message_length（payloadのWord数）
bits 9..12  : transfer_count
bits 13..14 : source（Normal=0、Fault=1、Notification=2、Reserved=3）
bit 15以降  : 通常の生成値では0
```

Normal messageの`message_length`にはMR0..3を含めず、request code自体は含める。8-bit fieldのためKernel ABI上の長さ表現は0..255である。これはNanami標準helperの「code + 4 arguments」という制限と同一ではない。Faultの現行length規約は例外があるため、後述の専用節を参照する。

通常のrequestでは`MessageInfo::normal(true, N, 0)`を使用する。replyは長さ3、Capability転送0で、単独REPLYでは`block=false`、REPLY_RECEIVEでは次の受信を待つため`block=true`である。

例えば、codeと4 argumentsを持つrequestのMessageInfoは`0x0b`、標準REPLYは`0x06`、標準REPLY_RECEIVEは`0x07`となる。

### 3.3 通常request / reply

```text
Request: [code, arg0, arg1, arg2, arg3]   length = 1..5
Reply:   [status, detail0, detail1]       length = 3
```

`call_service_port(port, code, arg0, arg1, arg2, arg3, message_length)`はこのrequestを送信し、replyの3 Wordsを返す。省略argumentには通常0を渡す。

このenvelopeには以下の共通fieldはない。

- wire上のservice名、interface ID、interface version
- correlation ID、request sequence number
- reply opcode、明示的なreply destination
- 全サービス共通のbuffer ID、session ID、deadline
- 型情報やserializer情報

operationを特定するのは接続先endpointとcodeである。例えばPCI、Exec、Negotoは別endpointで`0xd101`を使用し、RTCとHonokaも別endpointで`0x7001`を使用する。opcodeは大域一意ではない。

### 3.4 受信decoderの現状

Rustの`decode_service_event`は以下の順に解釈する。

1. FaultならFault専用fieldを読む。
2. NotificationならNotification eventを返す。
3. Normal以外ならProtocol error。
4. Normalなら、存在するpayload Wordsだけを読み、欠けたcode / argumentsを0にする。

`ServiceRequest`は元のmessage lengthやtransfer countを保持しない。5 Wordsを超える通常requestの追加payloadはこのdecoderでは公開されず、operationごとの厳密なarity検査も共通には存在しない。これは「任意のarityが正しい」という意味ではなく、現在の受信APIが情報を失うという制約である。

client helperは、Normalで長さ3以上のreplyを受理する。3 Wordsを超えるreplyは追加fieldを公開せず、返されたidentifierも認証結果としては利用していない。IDLで厳密なlength検査や追加fieldを導入する場合、既存互換動作とは区別する必要がある。

### 3.5 Kernel operationとservice methodは別番号

MR1のKernel operationと、MR4のNanami request codeは別の番号体系である。

| A9N IPC Port operation | MR1 | 必要な主な権限・役割 |
| --- | --- | --- |
| SEND | 1 | WRITE。送信のみで、reply待ち関係を作らない |
| RECEIVE | 2 | READ。requestまたはbound eventを受信する |
| CALL | 3 | WRITE。送信とreply待ち |
| REPLY | 4 | Kernelが保持するreply関係の完了。操作自体にはREAD / WRITEの追加検査はない |
| REPLY_RECEIVE | 5 | READ。replyを完了して次の受信へ進む |
| IDENTIFY | 6 | MODIFY。aliasのslot-local identifierを変更する |

現在の通常Nanami client helperはblocking CALLを使い、SEND用の一般的なoneway bindingは提供していない。`ServiceRequest`にも「SENDかCALLか」のfieldはない。IDLで新たにoneway methodを加える場合、現在のrequest/reply規約の単なる省略形として扱ってはいけない。

## 4. CALL、REPLY、通知割込み、失敗

### 4.1 同期requestの関係

- A9NのCALLは送信とreply待ちを組み合わせる。受信者が未準備なら送信queueで待ち、受理後はreply待ちになる。
- reply相手はKernelのreply関係に保持される。`identifier`を宛先としてREPLYする方式ではない。
- 受信側は未返信のrequestに対して1回のreplyを完了させ、その後に次の受信へ進む。現在のsequential runtimeに任意数のreply tokenを保存するAPIはない。
- `service_reply_receive_event`は現在のreplyを完了し、次のRequest / Notification / Faultを受け取る。戻り値がNotificationでも、元のrequestへもう一度replyしてはいけない。
- サービス処理中にAlphaや下位サービスへCALLすることはある。ただし循環する同期依存はdeadlockになり得る。一般的な再入可能RPC executorが存在するわけではない。

通常loopは`RECEIVE -> handle -> REPLY_RECEIVE -> handle ...`である。event駆動サービスは`service_receive_event`等を使う。`service_receive` / `service_reply_receive`はNotificationを読み捨ててNormal requestを待つため、IRQ、Timer、共有queue更新を処理するloopの代替にはならない。

### 4.2 CALL前のNotificationと安全な再試行

KernelのCALL slow pathには、受信者が未準備で送信queueへ入る前に、bound Notificationのpending値を返す経路がある。このとき返るのはreplyではなく`source=Notification`であり、requestはまだ配送されていない。

Rustの`call_port`はこの場合に限り次を行う。

1. 返されたidentifierを`PENDING_NOTIFICATION`へOR保存する。
2. request codeと全argumentをmessage registersへ書き直す。
3. CALLをやり直す。

これは「処理済みかもしれないrequestの自動再実行」ではない。受理済み・reply待ちのCALLを任意のNotificationで中断できる、という保証もない。

event loopへ戻ると保存済みbadgeがNotification eventとして返る。保存値があるときの`service_reply_receive_event`は、単独REPLYを実行してから保存済みeventを返し、既に消費した通知を再びKernelで待たない。保存値がなければcombined REPLY_RECEIVEを使用する。

現在の保存cacheとbinding記録はProcess内のglobal atomicであり、汎用のmulti-thread / multi-endpoint executorのためのthread-local event queueではない。同じNotification objectの別slot aliasでも、runtimeはdescriptor一致で保存値を引き当てる。grantとwaitには`bound_notification_slot()`で得たslotを使う必要がある。

### 4.3 エラーの二層

Kernel invocation failureはRustでは主に`RequestError::Transport`へ変換され、Kernelの`InvalidArgument`だけは`RequestError::InvalidArgument`となる。正常に到着したreplyのservice statusとは区別する。

| Nanami共通status | 値 |
| --- | --- |
| `OS_RESPONSE_OK` | 0 |
| `OS_RESPONSE_INVALID_ARGUMENT` | 1 |
| `OS_RESPONSE_PERMISSION_DENIED` | 2 |
| `OS_RESPONSE_INVALID_DESCRIPTOR` | 3 |
| `OS_RESPONSE_ILLEGAL_OPERATION` | 4 |
| `OS_RESPONSE_FATAL` | 5 |

サービスは独自statusも定義できる。例としてnetworkの`NET_SERVICE_RESPONSE_WOULD_BLOCK = 0x3000_0001`がある。`RequestError::Unsupported`などのローカルenum値は、このstatus表のwire値と同一ではない。Linuxの負のerrnoもNanami共通statusではない。

reply statusが非0でも、Shared Memoryの`detail0` bytesぶんに有効なエラー本文を返す場合がある。NegotoはJSONのerror responseをこの形で返すため、全errorでpayloadを捨てるbindingでは現行意味を保存できない。

### 4.4 完了・再試行・取消の保証範囲

- CALLの戻りは、operationで定義した同期処理または受理の完了である。物理デバイスへの永続化や非同期イベントの完了とは限らない。
- block WRITEとFLUSHは異なる。Timer予約のreplyも満了ではない。
- 接続・CALLに共通のtimeout、deadline propagation、application-level cancel、重複排除はない。
- transport failureやreply failureだけから「サーバーは実行していない」と判定できない。任意の自動再試行、接続し直しての再送、buffer再利用を生成してはいけない。
- PCBの停止・wait取消・Port revoke等はKernel管理操作であり、一般的なrequest取消protocolとしては公開されていない。Port revokeやProvider故障を、全reply waiterへの自動エラー返信と同一視しない。

### 4.5 C++ bindingとの差

現行の小さなC++ runtimeも同じ通常request/reply envelopeを使用する。ただし`cpp/src/nanami/nanami.cpp`の`call_port`は、RustのようなNotification保存・CALL再試行loopを持たず、非Normal応答を`Unsupported`へ変換する。またservice statusの変換もRustより情報量が少ない。低位のCALL実装も現在はx86_64のregister / syscall配置に対応するものなので、全言語bindingが全architectureを実装済みとはしない。

したがって、Rust/C++ bindingの既存実装が全て同じevent動作を持つとはしない。IDL Compilerではwire互換性と、各言語runtimeの実装済み機能を分けて扱う必要がある。

## 5. 名前解決とendpoint権限

### 5.1 既存service登録・接続

service名はUTF-8の1..24 bytesを、3つのWordにLE packingする。短い名前は0 paddingし、Alphaは最初のNULまでを名前として読む。24 bytesちょうどの場合は終端NULを必要としない。NULを含む名前を別名として扱う仕組みはない。

以下の表のargumentsはcodeを除いた順序である。reply欄は成功時の`detail0, detail1`を示す。

| Alpha request | code | arguments | reply |
| --- | --- | --- | --- |
| SERVICE_REGISTER | `0x1003` | name0, name1, name2, service_slot | 登録ProcessのPID, 0 |
| SERVICE_CONNECT | `0x1005` | destination_slot, name0, name1, name2 | Provider PID, 0 |
| SERVICE_LIST | `0x1010` | ordinal, chunk | owner/名前長、または名前chunk |

Native Processには初期service endpointがslot 20に作られる。登録はこのような既存endpointに名前を付けるもので、文字列名そのものがKernel IPCの宛先になるわけではない。一つのendpointに複数のservice名を登録することもある。

Alphaは接続先aliasへ呼出元PIDを`IDENTIFY`で設定し、READ|WRITEを残してCOPY / MODIFYを落とす。OS Portのcaller identifierも同様に固定される。payloadに自己申告のPIDを置くこととは異なる。

重要な制約として、既存接続のaliasはWRITE-onlyではなくREAD|WRITEである。identifierの改変防止とreceive権限の分離は同一の保証ではない。legacy API群を同一Alpha内の完全な相互不信sandboxと解釈しない。

名前の上書きで既存endpointを付け替える仕組みはない。同じowner / endpointでの登録再試行以外の重複は拒否される。registryの上限は64件で、Provider終了後の古いdescriptorから別Processへ誤接続しないための生存確認も行われる。

### 5.2 Identifierはbody fieldではない

`ServiceRequest.identifier`はKernelがaliasから受信側に渡した値である。通常の既存serviceではPID、Managed ServiceではSession badgeになる。IDLのgenerated dispatchへは、callerが自由に指定できるargumentとは分離した受信contextとして渡す必要がある。

Capabilityのobject identity、slot-local identifier、rightsはそれぞれ異なる。IDENTIFYの変更にはMODIFY権限が必要である。同じobjectのaliasでもidentifier / rightsは異なり得る。

## 6. Capabilityの付与と転送

現行の通常Rust helperは常に`transfer_count=0`であり、Shared MemoryやNotificationの準備にKernel IPCのCapability転送配列を使ってはいない。

通常の準備方法は、Alphaへ「相手PID、相手のslot番号、自分の設置先slot番号」等をrequestし、AlphaがNode操作でCapabilityを設置するものである。slot番号を送信すること自体がCapability transferではない。

A9Nの低位IPCには別途Capability転送機能がある。

- 送信側IPC bufferの`transfer_source_descriptors[]`に転送元を設定する。
- 受信側IPC bufferに転送先Node descriptorと開始indexを設定する。
- `transfer_count`件を連続した転送先slotへ**MOVE**する。自動COPYではない。
- 配列は16件だが、現行MessageInfoのcount fieldは4-bitなのでwire上は0..15件である。
- 複数件を順次MOVEする実装であり、途中失敗時の全件atomic rollbackは契約されていない。

この低位機能を将来IDLへ公開するなら、通常のAlpha-mediated grantとは別transport effectとして定義する必要がある。現行標準helperで任意のCapability引数を送れるとはしない。

## 7. Shared Memory

### 7.1 新規共有領域と既存allocationの共有

以下はAlphaに対する通常CALLである。

| 操作 | code | arguments | 成功replyのdetail0, detail1 |
| --- | --- | --- | --- |
| SHARED_MEMORY_CREATE | `0x1008` | peer_pid, size_bytes | caller側VA, peer側VA |
| MAPPING_SHARE | `0x1029` | peer_pid, owned_base_va, size_bytes | 元のcaller側VA, 新しいpeer側VA |
| MAPPING_RELEASE | `0x1014` | base_va, size_bytes, target_pid | 0, 0 |
| SHARED_FRAMEBUFFER_CREATE | `0x100e` | peer_pid, physical_address, size_bytes | caller側VA, peer側VA |

`MAPPING_RELEASE`の通常の自己解放helperはtarget_pid=0を送る。他Processの解放を行うAPIは別の権限検査を伴い、単に任意のPIDのmappingを消せることを意味しない。

`SHARED_MEMORY_CREATE`は新しいRAM backingを割り当て、page単位に切り上げ、両参加者へmapする。再利用physical pageから前ownerの内容が漏れないようにzeroingする。caller / peerのVAは異なり得るため、replyの二つのVAを同じpointerとして扱ってはいけない。

`MAPPING_SHARE`は既にcallerが所有するactive RAM allocationを追加共有する。既存内容とcaller mappingを保持し、対応するpeer mappingを返す。

- peerとcallerは別Processで、activeな参加者でなければならない。
- 元VAはpage-alignedである必要がある。
- page-roundedの指定範囲が、記録されたallocation全体と一致しなければならない。部分領域や任意のphysical addressを公開するAPIではない。
- frame capabilityとVM記録を確認し、途中失敗時は設置済みmapping / Capabilityをrollbackする。
- 通常のRAM共有はNormal/WBである。DMA RAMの共有も、CPU cache属性を無条件にWCへ変更することではない。

`SHARED_FRAMEBUFFER_CREATE`はphysical framebufferを扱う別APIであり、通常RAM allocationの共有とは区別する。MMIO / framebuffer / DMAのcache属性やdevice coherencyは、各driverとmapping契約の責任である。

### 7.2 サービスが共有領域を準備する場合

VFS等のattachでは、clientは相手PIDを知る必要がない。serverが受信identifierからclient PIDを得て、Alphaに共有領域を作らせる。

```text
Client -> VFS: CONTROL(ATTACH_SHARED_MEMORY, requested_bytes)
VFS    -> Alpha: SHARED_MEMORY_CREATE(client_pid, bytes)
Alpha  -> VFS: (OK, vfs_local_va, client_va)
VFS: client_pid -> (vfs_local_va, logical_bytes) を保存
VFS    -> Client: (OK, client_va, logical_bytes)
```

以後のrequestは、保存した領域に対するoffsetとlengthを送ることが多い。物理的なpage切上げサイズと、protocolがclientへ公開する論理capacityが一致するとは限らない。

attachの上書き、同一clientによる再attach、再試行のidempotency、detachの有無はサービスごとに異なる。汎用の「attachを再送すれば同じbufferになる」という保証はない。

### 7.3 Pointer、offset、handleの区別

典型的なbuffer参照には次の種類がある。

1. **offset-based**: 接続時に共有した領域の中の位置。VFS READ / WRITE、block READ / WRITE、network payload、terminal等。
2. **peer-local VA**: Alphaやserverがmapした結果として返す、受取人のaddress spaceで有効なVA。attach reply、GPU CREATE、Honoka surface等。
3. **owner-scoped handle + range**: GPU buffer IDとrectangle、window IDとsurface操作等。
4. **明示的なProcess memory操作**: 対象PIDとVAを指定しAlphaの権限付きcopy APIを使う。Shared Memoryのoffsetとは異なる。
5. **delegated region**: requestするownerと、実データを保持するpeerが異なる。VFSのdelegated I/O等。

`buffer_addr`や`local_vaddr`のような変数名だけではwireの意味を判別できない。IDLには「どの領域・誰のVA・どの単位」の参照かが必要である。

### 7.4 Lifetimeと同期

- 一方のmapping releaseは、相手側mappingが既に無効になったことを意味しない。通常RAMは、両側のallocation参照とmappingの解放が完了するまでbackingを保持する。
- server handleの破棄とclient側mappingのreleaseは別操作の場合がある。GPUの`destroy` helperはserver DESTROY成功後にclient mappingをreleaseする。
- Shared MemoryはCALL時にsnapshotされない。別coreのclientはserver処理中にも変更できる。可変長descriptor等の信頼境界では、検証する値のlocal snapshotなどが必要になる。NegotoはJSON commandを先にlocal bufferへcopyしてからparseする。
- 同じ領域を複数のoutstanding requestが同時に書くための一般的なlease / allocatorはない。buffer layoutが別途並行利用を許可しない限り、call中にその入力・出力範囲を再利用しない。
- Shared Memoryの共有自体は、キューのpublicationやCPU/device間の可視性を定義しない。atomic ordering、fence、DMA cache maintenance等は各通信方式で確認する必要がある。
- timeout、DMA完了不明、mapping撤回失敗の際に、解放や再利用を成功扱いしてはいけない。GPU driverは不明なin-flight memoryをpinし、Honokaはそのbufferへの描画を停止する。

Managed bufferは別の所有権規則を持つ。通常の`MAPPING_RELEASE`で自由にfreeするallocationではない。後述のprofileの管理手順を使用する。

## 8. Notification

### 8.1 Payloadを持つevent messageではない

NotificationのNOTIFYは、対象aliasに設定されたidentifierをpending値へORする。任意のpayloadやrequest bodyを渡すAPIではない。

```text
notify(alias_with_badge_A)
notify(alias_with_badge_B)
notify(alias_with_badge_A)
wait() -> A | B
```

- 回数と順序は復元できない。
- WAITはpending値を消費して返す。pendingがなければ停止する。
- POLLは非blockingで、通知がなければ0を返す。
- NOTIFYは非blockingな起床hintであり、相手が実処理を完了したことを意味しない。
- zero identifierは有用なpending flagにならない。初期Notification自身のidentifierが0でも、通知元のaliasには非0 badgeを設定して使用する。
- badgeに小さな整数IDをORすると、複数IDが混ざって別IDと区別できなくなる。source enumやstream中のevent番号として一般化してはいけない。

正確なevent内容、順序、完了件数は共有queueやサービス状態から得る。Notificationだけで任意のrecordを配送する設計ではない。

### 8.2 Alphaによる準備

| Alpha request | code | arguments | effect |
| --- | --- | --- | --- |
| NOTIFICATION_PORT_CREATE | `0x100c` | callerのdestination_slot, identifier | 新しいNotification objectを作る |
| NOTIFICATION_PORT_COPY | `0x100d` | source_pid, source_slot, callerのdestination_slot, identifier | 同じobjectのaliasをcallerへ設置し、送信用identifierを設定する |

例えばserverからclientへ通知する場合、serverはclientのNotificationを自身のslotへcopyし、そのaliasへ通知用badgeを設定する。逆方向の場合はclientがserverのobjectのaliasを持つ。

Alphaはsourceがlive Native Processの記録済みNotificationであることや、COPYが許可されることを確認する。未知のCapabilityへNotification用opcodeを投げてtypeを推測する方式ではない。

legacy copyは常にWRITE-onlyのsignal権限へattenuateするAPIではなく、sourceの権限を継承する。Managed EventのWRITE-only client aliasとは区別する。slot番号・peer PIDがpayloadで渡される場合も、service requestの認証済みcaller contextと同じ意味ではない。

### 8.3 Bound Notificationとservice loop

Native Processは共通Notificationを持ち、Alphaが初期bindingを行う。runtimeは`bind_current_thread_notification(descriptor)`でPCBのbindingを変更できる。一つのNotification objectは同時に別々のProcessへbindする用途には使えない。

bound Notificationは、IPC RECEIVEで停止中のProcessを起こせる。event loopでは`MessageInfo.source=Notification`、`identifier=pending_badges`、payload length=0として観測される。従ってRustの`ServiceEvent::Notification.value`はこの経路では0である。`value`を自由な通知payloadと見なしてはいけない。

直接Notification WAITで待つ経路もあり、その場合はbadgeがWAITの結果となる。これはIPC上の通常requestの受理ではないためREPLYしない。

binding変更はruntimeの保存済みbadge cacheをclearする。複数の通知objectを切り替えるgeneratorは、旧sourceのdrain・保存・再scanを含むstate machineを持つ必要がある。単にaliasの数だけ独立した`await event`を並べられるruntimeではない。

### 8.4 主なbadge

下記は現行の定数であり、全アプリケーション共通の排他的なbit allocatorではない。同じ受信Notificationへ異なる機能を集約する場合、その組み合わせで衝突しないことを確認する。

| 意味 | 値 |
| --- | --- |
| Timer | `1 << 63` |
| Managed binding change | `1 << 61` |
| Managed resource rescan | `1 << 60` |
| Terminal input | `1 << 52` |
| Terminal output | `1 << 51` |
| Input driver queue更新 | `1 << 50` |
| Input subscriber / Honoka input | `1 << 49` |
| Honoka present | `1 << 48`。legacy clientはlow 32 bitsにwindow IDもORする |
| Network RX / readiness | `1 << 20` |
| Managed work | `1` |
| IRQ | 対応するIRQ番号の`1 << irq_number`。AlphaはWord幅未満か検査する |

Managed SessionIdの最上位bitは通常requestのidentifier用であり、Timer badgeの最上位bitと同じ通信チャネルのbit割当ではない。

### 8.5 Queueとreadinessの取り扱い

consumerはbadgeを1件のデータと解釈せず、対応する共有queueやreadinessを確認し、残っている仕事を処理する。キューを処理しきれずbudgetを使い切った場合、badgeを消費した後で新通知が来ないことを前提に、残りの仕事をlocal pendingとして保持する必要がある。

sleepに入るときは次の関係が必要である。

```text
通知先をattachする
  -> queue / readinessを確認する
  -> 未処理仕事があれば処理する
  -> なければ、保存済み通知も含めてWAIT / RECEIVEする
  -> 起床後に状態を再確認する
```

先に状態確認してから通知先を登録すると、その間に起きた変化を失う可能性がある。Alter/Linuxのpoll待機は、先にNotificationをattachし、readiness query中に来たbadgeをruntimeが保存することでcheck/parkの間を扱っている。

IRQではdevice側の割込み要因処理と`interrupt_ack`が別途必要である。通常event用の自動REPLYをIRQ ACKの代わりにはできない。

## 9. 実在する複合通信パターン

### 9.1 VFS / block: bulk data + 同期制御

VFSの代表例は次のとおり。payloadは`[code, arguments...]`である。

| request | payload | length | 成功reply |
| --- | --- | --- | --- |
| attach | `0x9101, 1, bytes` | 3 | `OK, client_va, capacity` |
| open | `0x9102, path_offset, path_bytes` | 3 | `OK, file_handle, 0` |
| read | `0x9103, handle, file_offset, length, output_offset` | 5 | `OK, bytes_read, 0` |
| write | `0x9109, handle, file_offset, length, input_offset` | 5 | `OK, bytes_written, 0` |
| close | `0x9105, handle` | 2 | `OK, 0, 0` |

pathやread/writeデータはattach済み共有領域に置く。file offsetとSHM offsetは別物である。ownerは受信identifierから得て、handleとbufferを対応付ける。

block READ / WRITEはblock index、block count、SHM byte offsetを送る。block sizeはGET_INFOの結果であり、countをbytesとして扱ってはいけない。block WRITEのreplyはdevice cacheへの書込みを含み得るため、永続化が必要なら別のFLUSH契約を使う。

VFSのdelegated I/Oでは、file handleのownerとbufferのpeerを分離する。attach結果はpeer側VAと`delegate_id << 32 | size`を返し、後続requestでは`delegate_id << 32 | shm_offset`を用いる。descriptor、PID、handle、共有領域の役割が一つの整数型に見えても異なる実例である。

### 9.2 Timer: 受理reply + 後続Notification

| request | payload | 直接replyの意味 |
| --- | --- | --- |
| sleep予約 | `0x4002, milliseconds, notification_slot` | 予約の受理 |
| interval | `0x4003, milliseconds, notification_slot` | interval登録の受理 |
| clock query | `0x4004` | `OK, ticks, tick_hz` |
| alarm | `0x4005, absolute_tick, notification_slot, enabled` | alarmの設定・取消の受理 |

古い`0x4001`のsleep requestも現在は予約を受理して返し、満了をNotificationで知らせる。server自身を満了までblockingさせる方式ではない。

timer-serverはclientの指定Notificationのaliasを保持し、Timer badgeで満了を通知する。slot=0は共通slot 21を選ぶ。clockの単位はqueryしたtick_hzで決まり、TSCそのものやmillisecondsと固定してはいけない。

同じNotificationには複数のsleep / interval / alarmが同居し得る。badgeだけではどの予約が完了したか分からない。共有bound Notificationを使うsleep helperはclockとdeadlineを再確認し、無関係なbadgeを後でevent loopへ戻す。

専用delay用Notificationでは、その用途以外のinterval / IRQ等を混ぜないという追加前提がある。alarmの取消後でも既に配送済みbadgeは取り消せず、intervalの取りこぼし回数もNotificationからは得られない。

### 9.3 Input: 共有ring + 起床hint

`SUBSCRIBE_SHARED = 0x6104`はevent maskを受け、client側queue VAとbytesを返す。input-serverは共有queueの準備と、client Notificationへのaliasの準備を行う。

`ATTACH_DRIVER_SHARED = 0x6105`ではdriverがqueueへeventを書き、input-serverがconsumeする。subscriber向けではinput-serverがproduceし、Honoka等がconsumeする。request / replyの方向だけでは実データの流れを表せない。

Input queueは16 KiB、Word-alignedで、次のlayoutを持つ。

| Word index | 内容 |
| --- | --- |
| 0 | magic=`0x494e_5055_5451_3031` |
| 1 | capacity。通常1024 |
| 2 | head（consumer cursor） |
| 3 | tail（producer cursor） |
| 4 | dropped件数 |
| 5以降 | 1 Word / eventのring |

head / tailはmodulo capacityで扱い、空きを1 slot残す。満杯なら新eventをdropしdroppedを増やす。現在の実装はatomic WordアクセスにSeqCstを使用し、consumerのhead更新にCASを使う。producerは単一である。汎用MPMC queueとしての契約ではない。

event Wordのpackingは次のとおり。

```text
bits 0..7   kind
bits 8..23  code
bits 24..39 value0（i16のbit pattern）
bits 40..55 value1（i16のbit pattern）
bits 56..63 flags
```

kindにはkey、mouse button、move、wheel、window close、resizeがある。queueのmagic / headerは、このInput protocolのものであって全共有queueの共通headerではない。

### 9.4 Honoka: surface、damage、入力、resize mailbox

window clientの一例は以下の準備を行う。

1. `honoka-service`へ接続し、windowをcreateする。
2. HonokaのNotificationのaliasをclientへcopyし、`PRESENT | window_id`を設定する。
3. logical framebufferをattachし、client側VAとbytesを受け取る。
4. window input queueをattachする。
5. client Notificationへのinput通知をattachする。

logical framebufferの先頭4 KiBはdamage queue用で、pixel dataは`base + 0x1000`から始まる。damage queueはmagic、capacity、head、tail、補助flagの5 Wordsと、1件あたり`x, y, width, height`の4 Wordsを持つ。capacityは64である。Word 4はInput queueのdroppedと同じ意味ではなく、Honoka consumerには先頭rectangleを使う補助更新経路がある。

clientはpixelを書き、damage entryを公開した後にNOTIFYする。`nanami-ui::Window`のproducerはentryをRelaxed storeし、tailをRelease storeする。consumerはatomic accessで公開済みentryを読む。queueが使えない・満杯の場合、同helperは`INVALIDATE_LOGICAL_FRAMEBUFFER = 0x7010`の同期requestへfallbackする。

legacy present badgeのlow bitsはwindow IDであり、複数windowの通知をORするとIDが混ざり得る。実装には定期Timerで全windowのdamage queueをdrainする経路もある。IDLではこのbadgeを「正確に一つのwindow event」としてモデル化してはいけない。

input領域には通常のInput ringに加え、`Word[5 + 1024]`にresize mailboxがある。最新resizeを上書きし、consumerはatomic swapで取り出す。満杯のkey/mouse queueに最終resizeを失わせないための、ordered ringとは別のlast-value方式である。

surface resizeは同期操作であり、clientは描画を止めてrequestし、成功後は新しいmapping / dimensionsだけを使用する。旧mappingのreleaseとserver側の新surfaceへの切替は別のlifecycleである。単なる可変長戻り値としてpointerだけを置き換えると、旧領域の解放責任を表せない。

### 9.5 Network / terminal: readiness hint + 再query

networkとterminalは、bulk transferにclientごとの共有領域を使い、受信可能性の変化をNotificationで知らせる。readinessのquery、実際のread / recv、buffer contentsは別の情報である。

- Notificationを受けても、他のconsumerや状態変化によりread時にデータがないことがある。
- networkには、owner-scoped readinessとWOULD_BLOCK statusがある。通知だけからbytes数やsocket IDを復元しない。
- network backendのRECV_BATCHは、共有領域に`LE u32 length + frame bytes`の固定stride recordを最大32件書く。InputのWord ringとは別schemaである。
- terminalのinput / output通知は別badgeである。terminalのByteRing自体はserver内部状態であり、clientの共有transfer bufferがそのまま共通ringとして公開されるわけではない。

### 9.6 GPU: server所有handle + CPU共有mapping

GPU protocolはABI version 1で、GET_INFO / CREATE / DESTROY / FILL / COPY / PRESENTを持つ。CREATEはbuffer IDとclient VAを返す。IDはownerとproviderにscopeされ、VAはCPU access用である。GPU addressやvendor command packetをclientから受け付けるprotocolではない。

FILL / COPY / PRESENTはそのbackendの操作完了を待って返す同期操作である。現行仕様にはasync fence handleやcommand submission queueはない。`supported`と`accelerated`は別で、software fallbackが同じmessage interfaceを実装する。

Honokaは既に選択されたdisplay endpointのcapability flagを確認し、その同じendpoint上でGPU protocolを使う。service名を別々に解決して、表示と描画が異なるproviderになってよいわけではない。

詳しいlayout / packing / failure時のpin規則は[GPU specification](../docs/gpu.md)を参照する。

### 9.7 Negoto / Alter: opaque byte payloadにも複数形式がある

Negotoはclientごとに64 KiBの共有領域をattachする。COMMAND=`0xd102`のarg0は領域先頭のUTF-8 JSONのbyte長で、最大1024 bytesである。replyは同じ領域をresponse JSONで上書きし、detail0にresponse byte長を返す。従ってこの領域はin/outであり、request内容はreply後にも保存される契約ではない。

Alterのexecution-context APIは別形式である。48 bytesのheaderに`ABI, flags, root_len, cwd_len, executable_len, launch_len`をそれぞれLE u64で置き、その後に各byte列を連結する。過不足・trailing data・未定義flagを検査する。launch部分はさらにargc / envcとNUL終端argv / envpという既存形式を持つ。

`flags`のbit 0はreadonly root、bit 1は明示的なgraphics grantである。NegotoはOCI annotation `io.nanami.graphics="true"`をbit 1へ変換する。未指定・`"false"`ではgrantしない。graphicsはreadonlyとは独立で、通常のdevice passthroughやhost inputの購読権限を意味しない。Alterは実行contextごとのHonoka window、論理framebuffer、window-local input queueに接続する。forkしたX server/client群はcontext leaderを基準に同じgraphics sessionを共有する。framebuffer共有と入力Notificationは既存Honoka protocolを使用し、Kernelのcapability transfer契約は変更しない。

つまり「共有byte配列」という型だけでは、JSON、固定header付き可変長record、複数のNUL文字列という違いを表せない。

## 10. Managed Service

### 10.1 四つの管理対象

| 対象 | 意味 |
| --- | --- |
| Service | Alphaが所有するpublic名と安定endpoint |
| Provider | interface token、profile、受信slot、管理Notificationを登録したProcess |
| Session | clientがServiceを利用する権限と、固定されたidentifier |
| Binding | Serviceと現在のProviderの対応、generation、切替phase |

Managed clientはWRITE-onlyのendpoint aliasを受け取り、ProviderはCOPY / MODIFYを持たないREAD|WRITE aliasを受け取る。clientのSession badgeは`SESSION_IDENTIFIER_BIT | (record_index + 1)`であり、現在の64-bitでは最上位bitが立つ。

interface tokenはProvider間の互換性を検査するopaque Wordである。IDLのschema hashやservice名から自動生成されたversionを意味する規約は現在ない。切替時はinterfaceとprofileの両方が一致する必要がある。

登録・管理操作はAlphaへのCALLだが、通常requestはclientから安定endpointへ直接CALLする。request bodyにSessionIdを毎回足すのではなく、受信identifierがSessionを識別する。

### 10.2 登録・接続のwire

| Alpha request | code | arguments | 成功replyのdetail0, detail1 |
| --- | --- | --- | --- |
| PROVIDER_REGISTER | `0x1025` | interface, receiver_slots, management_notification_slot, profile | ProviderId, 0 |
| SERVICE_PUBLISH | `0x1026` | name0, name1, name2, ProviderId | ServiceId, 0 |
| SESSION_CONNECT | `0x1027` | destination_slot, name0, name1, name2 | SessionId, ServiceId |
| BINDING_CONTROL | `0x1028` | operation, id, parameter0, parameter1 | operation固有 |

Provider登録はProcess自身の操作である。publishやbinding管理には、launch policyの`services=1`等の管理権限が必要で、そのServiceを作ったmanagerが後の切替を認可する。名前解決の結果にProvider PIDを返すAPIではない。

legacy SERVICE_CONNECTでManaged名へ接続することは拒否される。legacyのRW aliasを誤って付与しないためである。

`BINDING_CONTROL`のsuboperationは次のとおり。

| 番号 | operation | id / parameter0 / parameter1 |
| --- | --- | --- |
| 1 | PROVIDER_LOOKUP | local_pid / 0 / 0 |
| 2 | PROVIDER_BINDING | ProviderId / 0 / 0 |
| 3 | PREPARE | ServiceId / expected_generation / next_ProviderId |
| 4 | QUIESCE | ServiceId / expected_generation / manager_notification_slot |
| 5 | QUIESCED | ProviderId / expected_generation / 0 |
| 6 | COMMIT | ServiceId / expected_generation / 0 |
| 7 | ABORT | ServiceId / expected_generation / 0 |
| 8 | STATE | ServiceId / 0 / 0 |
| 9 | CLOSE | SessionId / 0 / 0 |
| 10 | PROVIDER_RETIRE | ProviderId / 0 / 0 |
| 11 | BUFFER_CREATE | SessionId / bytes / 0 |
| 12 | BUFFER_VIEW | SessionId / 0 / 0 |
| 13 | EVENT_CONNECT | SessionId / destination_slot / 0 |
| 14 | PROVIDER_SESSION | ProviderId / ordinal / 0 |

binding queryは`ServiceId, (generation << 3) | phase`を返す。phaseはActive=0、Prepared=1、Quiescing=2、Quiesced=3、Faulted=4である。generationはapplication requestのsequence numberではない。

### 10.3 Profile

| profile | 値 | 保持するもの | 含まないもの |
| --- | --- | --- | --- |
| STATELESS_CONTROL_PROFILE | 1 | endpoint / Session / 協調切替 | application共有状態、event subscription、device state |
| MANAGED_BUFFER_PROFILE | 2 | 上記 + Sessionごとのopaque RAM buffer | queue schema、private heap、device DMA |
| MANAGED_EVENT_PROFILE | 3 | 上記 + Service所有のpending-work Notification | event recordのschema、一般的な双方向event broker、device handoff |

profile 1/2の`receiver_slots`は単なるreceiver slotである。profile 3では`receiver_slot | (event_slot << 8)`を送る。receiver / management Notification / eventの各slotは23..223かつ別々である。

### 10.4 協調的なProvider交換

```text
Active(g)
  -- PREPARE(next) --> Prepared(g)
  -- QUIESCE -----> Quiescing(g)
  -- old ProviderのQUIESCED ACKと権限撤回 --> Quiesced(g)
  -- COMMIT ------> Active(g+1, next)
```

1. PREPAREはmanager、generation、後継Providerの空き、interface / profile互換性を検査する。この時点で受信者は変わらない。
2. QUIESCEは旧Providerへ管理Notificationを送る。
3. 旧Providerは**受理済みCALLへのreplyを完了してから**ACKする。replyの成否不明ならACKせず、requestを再実行しない。
4. Alphaは旧ProviderがACK用のOS CALLで停止している間に、profileで追跡するevent / data / receive権限を撤回する。
5. COMMITは後継へ必要なmappingと権限を設置し、その後にgenerationを進める。
6. 旧Providerはretire手順に進む。

待機中の未受理CALLは同じKernel endpointに残る。Quiesced中の新CALLも受信者を待つ。managerがcommit / abortしない場合に、自動timeoutで別Providerへ移す仕組みはない。

ABORTはPreparedまたはQuiescedから行う。後者では旧Providerの権限と元のVAを復元してからActiveへ戻る。不明な撤回・rollback失敗はFaultedとして扱い、同時に二つのreceiverを有効にしない。

Rustのsequential `Receiver`はbindingをcacheし、通常requestごとにAlphaへ問い合わせない。継続したCALL列で管理通知が飢餓しないよう、32回のreceiveごとに非blocking Notification pollを行う。

### 10.5 Managed buffer

- Sessionにつき1個、最大64 KiB、page-rounded、zero-initializedのopaque bufferを持てる。
- `BUFFER_CREATE`はclientが呼び、自分のVAとbytesを得る。active Providerは受信Session badgeを使って`BUFFER_VIEW`から自分のVAを得る。
- 同じpage数でのcreate再試行は既存viewを返す。通常のlegacy attach全般にこの性質があるわけではない。
- AlphaはFrame capabilityをclient / Providerへ配らず、自分で保持したbackingをRW / non-executableでmapする。
- queueのlayout、cursor、magic、互換な状態の検証はProviderが定義する。Alphaはserializerでもqueue実装でもない。
- Quiescingでは旧Providerが受理済み仕事のためにVIEWすることはできるが、新規client allocationは拒否される。
- Quiesce時に旧Providerのdata mapping / copy aliasを撤回する。commit時は全bufferを後継へmapしてからreceive権限を渡す。
- 成功した交換ではclient側VAと内容を保持する。後継Provider側のVAは改めて取得する。
- 同profileのbufferを持つSessionのcloseは、現状ではService全体がQuiescedの場合に限られる。通常allocationのrelease APIでfreeする契約ではない。
- 現行のclose / retireにはCapabilityやbackingを隔離して保持する経路がある。close成功を、quota・全物理memory・全slotの即時回収保証と解釈しない。

上限はAlphaあたりProvider 32、Managed Service 32、Session record 64である。Session recordやclose後のquotaが直ちに再利用される契約ではない。buffer backingの上限は64件×64 KiBであり、管理用page table等は別に必要となる。

### 10.6 Managed Event

profile 3にはService所有のNotificationがあり、clientは`EVENT_CONNECT(SessionId, slot)`でWRITE-only aliasを得る。active Providerが受け取るobjectはそのServiceのもので、Provider個人のNotification objectをclientが追従し直す方式ではない。

- 通常workのNOTIFYは直接配送で、AlphaへのCALLを挟まない。
- WORK、BINDING、RESOURCE_RESCANは同一objectの異なるalias badgeである。
- eventは「どのSessionに何件あるか」を表さない。Providerは保持するbuffer viewとcursorを調べる。
- resource rescan時には`PROVIDER_SESSION`でlive buffer Sessionを列挙できる。普通のwork通知ごとに列挙・Alpha queryする必要はない。
- 旧ProviderはACK前にprivate管理Notificationへbindし直す。AlphaもPCB内部のevent aliasを撤回して、root slotの削除だけで古い受信権限が残らないようにする。
- commit / abortではeventとdata viewを準備し、resource rescan hintをseedしてからreceive権限を設置する。途中でwork badgeが管理badgeと混ざっても、retained cursorから未処理仕事を見つけ直す。
- clientのcontrol handle、event handle、data VAは成功した交換中に変わらない。完了済みqueue entryを巻き戻したり再実行したりはしない。

これは協調的な単一consumerのwork-hint契約であり、任意のlegacy Notification subscriptionを移植する仕組みではない。

### 10.7 Domainをまたぐimport

Managed importは、親のregistry全体や生のProvider PIDを渡すのではなく、明示的なupstream Sessionを委譲する。子Alphaのlocal SessionIdはclose用のlocal handleであり、upstream Providerに見えるbadgeと一致する必要はない。下流clientは一つのupstream identityを共有する場合がある。

現行importはprofile 1のみで、profile 2/3やPIDベースのShared Memory / Notificationは自動でDomainを越えない。IDLで`SessionId == peer_pid`、または「connectの返り値は常にserver PID」と仮定すると、この経路を表現できない。

## 11. Fault IPC

Faultは同じIPC endpoint / event decoderに現れるが、通常のservice requestではない。Alter/Linux等のpersonalityはKernelが送るINVALID_KERNEL_CALLをsyscall処理の入口として使う。

### 11.1 専用wire形式

| MR | 意味 |
| --- | --- |
| 3 | Fault送信元に対応するalias identifier |
| 4 | reason |
| 5 | program counter |
| 6 | memory faultではfault address。INVALID_KERNEL_CALLではkernel call / syscall number |
| 7以降 | reason固有。INVALID_KERNEL_CALLではhardware context配列 |

reasonはNONE=0、MEMORY=1、MEMORY_INSTRUCTION_FETCH=2、INVALID_INSTRUCTION=3、INVALID_ARITHMETIC=4、INVALID_KERNEL_CALL=5、FATAL=6、ARCHITECTURE=7である。

**lengthにはNormalとの非対称性がある。** 現行KernelはFault生成時、`message_length`に最後の使用MRの次のindexを設定する。例えばmemory faultは8、INVALID_KERNEL_CALLは`7 + hardware_context_words`となる。これはNormal messageの「MR4以降のpayload Word数」ではない。Rust decoderもこの現行Fault規約に合わせ、context数を`length - 7`で求める。共通のNormal decoderで長さを解釈し直してはいけない。

hardware contextの最大Word数はx86_64で23、AArch64で35である。内容・順序はarchitecture固有で、FPU stateまでを含む汎用recordではない。

Rust `ServiceEvent::Fault`は一般化されたfield名を持つが、例えば`fault_address`はINVALID_KERNEL_CALLではsyscall numberである。INVALID_INSTRUCTION等の短いFaultは別MR配置を持つため、`fault_address` / `architecture_fault_code`の全fieldが全reasonで有効とはしない。reasonごとの低位配置を参照する必要がある。

### 11.2 Faultへのreply

Fault continuationは`[status, detail0, detail1]`の通常replyと同じ契約ではない。

- Alphaの通常page fault処理には、payload length=0のreplyで元のcontextを継続する経路がある。
- `service_fault_continue_receive_event`は、MR4以降に更新hardware contextを置き、長さを更新Word数としてreplyする。
- INVALID_KERNEL_CALLのreplyは、Kernelがそのcontextをfaulting Processのregisterへ適用してから実行を再開する。
- Alterは一部のsyscall待機でProcessをparkし、後でcontextを書いてPCB RESUMEする。この場合のreply関係の処理も通常serviceの遅延reply用tokenとは異なる。

`MessageInfo.source`はdecoder用のtagであり、それだけをsenderの権限証明としては使わない。通常IPCのmetadataは送信値も扱うため、Kernel由来Faultとして解釈するendpointには、適切なresolver / Capabilityの付与規則が必要である。

IDLでFault handlingを対象に含めるなら、Normal RPC、Notification、Fault continuationを別のtransport形態として記述する必要がある。Linuxのsignal / EINTR / SA_RESTARTそのものはpersonalityの上位仕様であり、このNanami共通envelopeに自動的に含まれない。

## 12. 既存interfaceの所在

全operationの定数・個別packingの正本は`nanami/servers/uapi/src/`とそのclient / server実装にある。本書はそれらを新しいIDLへ移すための共通仕様と実例であり、全methodの引数表を複製するものではない。

| 分野 | UAPI | 主な通信形態 |
| --- | --- | --- |
| Alpha OS | [os.rs](../servers/uapi/src/os.rs) | mapping、grant、Process管理、名前解決 |
| Managed Service | [service.rs](../servers/uapi/src/service.rs)、[alpha.rs](../servers/uapi/src/alpha.rs) | Session / Binding / buffer / event / import |
| Driver Manager | [device.rs](../servers/uapi/src/device.rs) | platform / device metadata、driver準備 |
| PCI | [pci.rs](../servers/uapi/src/pci.rs) | scalar / packed record、名前chunk |
| USB | [usb.rs](../servers/uapi/src/usb.rs) | scalar情報。block / inputは別interface |
| block | [block.rs](../servers/uapi/src/block.rs) | 共有transfer buffer、read / write / flush |
| VFS | [vfs.rs](../servers/uapi/src/vfs.rs) | 共有領域内offset、handle、delegated領域 |
| network / net-device | [net.rs](../servers/uapi/src/net.rs) | 共有buffer、batch record、readiness通知 |
| Timer | [timer.rs](../servers/uapi/src/timer.rs) | 予約ACKと後続Notification |
| RTC | [rtc.rs](../servers/uapi/src/rtc.rs) | packed scalar reply |
| display / GPU | [gfx.rs](../servers/uapi/src/gfx.rs)、[gpu.rs](../servers/uapi/src/gpu.rs) | shared surface、同期command、capability query |
| Input | [input.rs](../servers/uapi/src/input.rs) | 同期fallback、共有ring、Notification |
| Honoka | [gfx/honoka.rs](../servers/uapi/src/gfx/honoka.rs) | window handle、surface、damage ring、input、mailbox |
| Terminal | [terminal.rs](../servers/uapi/src/terminal.rs) | 共有transfer bufferとinput / output通知 |
| POSIX | [posix.rs](../servers/uapi/src/posix.rs) | 共有buffer、fd / Process管理、delegated direct I/O |
| Exec | [exec.rs](../servers/uapi/src/exec.rs) | path / argv / env payload、Process管理 |
| Alter | [alter.rs](../servers/uapi/src/alter.rs) | context record、legacy launch、Fault IPC |
| Negoto | [negoto.rs](../servers/uapi/src/negoto.rs) | in/out JSON共有buffer |
| Power | [power.rs](../servers/uapi/src/power.rs) | scalar request |

一つのserverが複数interfaceを同じendpointで受ける場合がある。例えばUSB serverはUSB情報とblock-deviceの要求を扱い、display backendはlegacy displayとGPUを扱う。IDL上のinterfaceとProcess / endpointの個数を一対一と固定しない。

## 13. IDL / Compilerで表現が必要な情報

以下は現行ABIを表現するための設計入力であり、新たなIDL構文の指定ではない。

### 13.1 Method単位の情報

| 情報 | 必要な理由 |
| --- | --- |
| 接続方式、endpoint / interface、opcode | opcodeのscopeとManaged / legacy接続の違い |
| request / replyのWord数とMR位置 | register-only経路、既存packing、Faultの例外 |
| bit field、signedness、単位、endian、reserved | u16 / u32 packing、tick、sector、byte offset等を区別 |
| implicit caller contextの種類 | PIDとSession badge、payload中のpeer PIDとの混同防止 |
| statusとerror時のpayload | 二層error、JSON error body、WOULD_BLOCK等 |
| 完了点 | 受理、queue投入、CPU可視、device完了、永続化の違い |
| 再試行・取消・idempotency | transport errorを未実行と誤認しないため |
| feature / version queryとfallback | GPU等は一律必須command集合ではない |

### 13.2 Resource単位の情報

- Capability / slot / descriptor / object / identifier / rightsを分離する。
- 共有領域のallocator、owner、参加者、論理bytes、page丸め、CPU属性、address spaceを示す。
- 参照をVA、offset、resource ID、delegated IDのどれで渡すか示す。
- handleのscope、再利用、close、detach、mapping release、Provider retirementの順序を示す。
- 成功時だけでなく部分失敗・完了不明時に、誰が何を保持するか示す。
- serverに配る受信権限とclientに配る送信権限、さらにNotificationの方向を別々に示す。

### 13.3 Data / Event単位の情報

- RPC-only、共有bulk transfer、ring、last-value mailbox、readiness query、Fault continuationを区別する。
- 共有schemaのmagic / version / alignment / header / stride / variable tailを定義する。
- producer / consumer数、cursorの所有者、atomic ordering、publication順、queue容量と満杯時policyを示す。
- event payloadの所在とwake hintを分ける。coalescing、drop、overwrite、batch、再scanを表現する。
- subscribe / attachからwaitまでの順序、通知先object、badge mask、combined badgeの処理を示す。
- 正確な件数や順序が必要なら、Notification以外のauthoritative stateを指定する。
- Timerやdeviceでは、起床hintとは別の完了predicate / queryを指定する。

### 13.4 Connection / state machine単位の情報

最低限、次のようなlifecycleをRPCの列とは別に表せる必要がある。

```text
Disconnected
  -> EndpointGranted
  -> BufferMapped / NotificationAttached
  -> Ready
  -> WorkPublished / RequestInFlight
  -> Completed または CompletionUnknown
  -> Draining / Quiesced
  -> ResourcesWithdrawn / Closed
```

この図は各サービスに新しい共通stateを要求するものではない。現在のattach、publish、wait、resize、handover、detachの前提を、bindingが保持・検査できる情報として表すための整理である。

生成codeが追加する抽象化によって、通常の小さいCALL / REPLY_RECEIVEを、毎回のheap allocation、共有memory attach、Alpha中継、追加handshake、汎用serializer往復へ置き換える必要はない。既存register packingとdirect endpoint経路をそのまま表現できることが重要である。

## 14. 現状の境界と、別途設計が必要な点

- 全interface共通のversion negotiationやschema discoveryはない。GPU ABIやAlter context ABIは個別の仕組みである。
- 通常requestの厳密arity、reserved field検査、attach再試行、handle lifetimeは統一されていない。
- Notificationは非counting / 非orderedで、汎用event payloadや相関IDを持たない。legacyのwindow ID混在badgeを正確なevent列へ読み替えない。
- Faultのlength / payload規約はNormalと非対称であり、reasonごとにも差がある。
- legacy APIは同一Domain内のPID依存で、Managed profileの権限分離・交換保証とは異なる。
- Managed profileは協調交換の範囲を定義する。private heap、device DMA、任意の非協調Provider crash、全resource即時再利用は含めない。
- 普通のNative runtimeのevent保存はProcess-globalで、任意のmulti-thread / multi-channel async executorを提供していない。
- raw Kernel Capability transferとAlpha-mediated grantは別物である。通常bindingで前者を使っていると仮定しない。
- 「同期replyが返る」「通知を受ける」「queueにrecordがある」「データが永続化された」は別の完了条件である。

これらをどう統一・拡張するかは、別プロジェクトで行うIDLの設計判断である。本書の作成では通信実装・Kernel ABI・サービス挙動を変更していない。

## 15. 実装参照と確認箇所

本書の共通仕様は、主に以下のコードを照合した。

- [Rust call helper / Alpha requests](../servers/libs/nanami-runtime/src/lib.rs)
- [Rust event decoder / reply loop](../servers/libs/nanami-runtime/src/ipc/service.rs)
- [Notification binding / deferred badges](../servers/libs/nanami-runtime/src/ipc/ports.rs)
- [Native descriptor / TLS](../servers/libs/nanami-runtime/src/ipc/tls.rs)
- [C++ binding](../servers/libs/nanami-runtime/cpp/src/nanami/nanami.cpp)
- [Alpha legacy grant / connect](../src/nanami_core/alpha/control_requests.rs)
- [Alpha shared mapping](../src/nanami_core/alpha/shared_memory.rs)
- [Managed binding state machine](../src/nanami_core/communication/bindings.rs)
- [Managed buffer / event ownership](../src/nanami_core/communication/buffers.rs)、[events.rs](../src/nanami_core/communication/events.rs)
- [Managed buffer mapping effects](../src/nanami_core/alpha/service_buffers.rs)、[event effects](../src/nanami_core/alpha/service_events.rs)
- [Managed sequential receiver](../servers/libs/nanami-runtime/src/service.rs)
- [Input queue](../servers/libs/nanami-client/src/input/service.rs)
- [Honoka surface / resize mailbox](../servers/libs/nanami-client/src/gfx/honoka/client.rs)
- [UI damage producer](../servers/libs/nanami-ui/src/window.rs)
- [Honoka damage consumer](../servers/apps/honoka/src/app/compositor.rs)
- [Timer reservation / notification](../servers/core-services/timer-server/src/timers.rs)
- [A9N IPC Port](../../spencer/A9N/src/kernel/capability/ipc_port.cpp)、[MessageInfo / opcodes](../../spencer/A9N/src/kernel/include/kernel/capability/ipc_port.hpp)
- [A9N Notification](../../spencer/A9N/src/kernel/capability/notification_port.cpp)、[OR accumulation](../../spencer/A9N/src/kernel/include/kernel/ipc/notification.hpp)
- [A9N Fault wire](../../spencer/A9N/src/kernel/include/kernel/interrupt/fault.hpp)

補足の設計・検証記録は[Architecture](../docs/architecture.md)と[GPU](../docs/gpu.md)にある。テストの主な入口は[tests/alpha](../../tests/alpha)、[tests/native-performance](../../tests/native-performance)、[tests/alter-linux](../../tests/alter-linux)である。

## 16. Alter VM / namespace拡張

上記revision以降のAlter改善で追加したoperation。いずれもNanamiのサービス
protocolであり、A9NのCapability ABIやIPC fastpathは変更しない。

### 16.1 Alpha VM操作

| Request | Code | `arg0..arg3` | 成功時detail |
| --- | --- | --- | --- |
| `PROCESS_MAPPING_PROTECT` | `0x102a` | target PID, VA, byte count, R/W/X bits | なし |
| `PROCESS_MEMORY_COMPARE_EXCHANGE` | `0x102b` | target PID, aligned VA, expected u32, desired u32 | 以前のu32 |
| `PROCESS_THREAD_CREATE` | `0x102c` | source PID, caller側PCB destination slot, priority, reserved=0 | 新しいtask PID |
| `PROCESS_FORK` | `0x102d` | source PID, caller側PCB destination slot, priority, reserved=0 | 新しいtask PID |

PROTECTのpermissionはbit 0=R、1=W、2=X。0ではleafだけを外し、backingとFrame
Capabilityは保持する。対象本人またはreaperが要求できる。CASはrobust futexの
owner-death処理に利用する。4-byte alignmentを検証し、Alphaのcopy windowにmapした
実backingへatomic compare-exchangeする。guest memoryのコピーによる疑似CASではない。

THREAD_CREATEはsourceのreaperだけが要求できる。新しいroot/PCBを生成し、既存
AddressSpaceをaliasとして接続する。Fault resolverには要求元のservice portを使用し、
新しいtask PIDをidentifierとして固定する。PCBはsuspendedで返すため、register/TLS/TID
設定の完了前に実行されない。VM所有者のroot/frame arenaは最後の利用taskのreapまで保持する。

FORKもsourceのreaperだけが要求でき、suspended PCBを返す。こちらは独立した
AddressSpaceとRAM backingを作り、sourceの現在のbacked pageとpermissionを複製する。
ELFや元のpathnameは参照しない。PROT_NONEの保持済みbackingは複製し、未populatedの
予約領域は空のままにする。LinuxのFD・signal・VMA・MAP_SHARED再接続はAlterの責任。
これはCOWではなくeager snapshotであり、他の実行中thread全体を原子的に停止する
operationではない。生成途中の失敗では新しいroot/VMを回収する。

既存operationの拡張:

- `PROCESS_MAP_ANONYMOUS`の`arg3`=0は従来のzero fill。非0では、callerのaddress
  spaceにある2 Word `[source VA, initialized bytes]`へのpointerを指定する。
  `arg0`=target PID、`arg1`=mapping bytes、`arg2`=固定VA（0なら自動割当）は維持する。
  sourceはcaller自身の領域で、initialized bytesはmapping bytes以下。callerはtarget
  本人かreaperでなければならない。Alphaがsourceを検証後、新しい各pageへ直接コピーし、
  残りだけをzero fillしてからpageを公開する。既存backingへの上書きは拒否する。
  ファイル読み取りの短い応答はAlter側で部分pageだけをbufferし、完全pageは直接渡す。
- `MAPPING_SHARE`の`arg3`=0は従来どおりpeer VAを割り当てる。非0は固定peer VA。
  固定先は要求元がreaperとして管理する対象に限り、重複mappingを拒否する。
- `SHARED_MEMORY_CREATE`でpeer PID=0は、要求元だけにreclaimable RAMを作成する。
  memfd backing用であり、通常のheap allocationのdeferred解放とは異なる。
- `MAPPING_RELEASE`の`arg3` bit 0は穴を許容するsparse release。他のbitは拒否する。
- `PROCESS_MEMORY_READ/WRITE`のbyte count最上位bitは`OS_MEMORY_COPY_USER_ACCESS`。
  下位bitが長さで、設定時は対象のR/W permissionも検証する。未設定ではsupervisorの
  image構築・debug用privileged copyを維持する。SDKの`*_checked` helperがこのbitを設定する。
- `PROCESS_MEMORY_CLONE`とpage-alignedな`PROCESS_MEMORY_COPY_WITHIN`は、指定範囲内の
  backingを持つpageだけを複製し、permissionも保持する。未populatedの予約領域は複製しない。

### 16.2 POSIX / VFSのrooted pathname操作

`POSIX_REQUEST_PATH=0xa13c`、`VFS_REQUEST_PATH=0x9116`は`MR2=descriptor offset`
という形で、接続済みshared buffer内の`PathRequest`を指定する。
descriptorは11個のWordからなる`repr(C)`構造体で、順序は次のとおり。

```text
operation, directory,
root_offset, root_bytes, path_offset, path_bytes,
target_offset, target_bytes, output_offset, output_bytes, flags
```

offsetはすべて接続先shared bufferのbaseからの相対値。path/targetはNULを含まない
byte列で、rootは物理VFS上のnamespace root。directoryはPOSIX側ではsessionのFD、
VFS側ではsessionが所有するdirectory handleであり、`Word::MAX`はrootからの解決を示す。
絶対pathはdirectoryを使わない。POSIXは入力を自分のVFS共有領域にコピーし、FDを
VFS handleへ変換して転送する。返却byte列も呼出元のoutput bufferへコピーする。

OperationはRESOLVE=1、SYMLINK=2、READLINK=3、STAT=4、CHMOD=5。
flagsのbit 0はfinal symlinkを辿る、bit 1は最後のcomponentの未存在を許容する。
bit 2（`VFS_PATH_WITH_METADATA=4`）はRESOLVE専用で、outputを4 Wordの
`PathMetadata { inode, size, mode, file_type }`と、それに続くcanonical pathにする。
output_bytesとdetail0はともにheaderを含むbyte数で、detail1とheaderのfile_typeは
従来どおりVFS file type。headerとpathの間にpaddingやNUL終端は入れない。
modeはfile typeを含むinode modeの全16bitで、permission bitsだけではない。
未存在の最後のcomponentを許容した場合はheaderを全0にし、canonical pathは返す。
このflagを指定しない既存clientのpath-only形式は変更しない。情報はその解決処理で
取得したsnapshotで、後続requestに有効なcacheや永続的なobject handleではない。
SYMLINKのtargetは保存するlink内容。bit 2未指定のRESOLVEのoutputはnamespaceから見たcanonicalな
絶対path、READLINKのoutputはlink内容であり、どちらもNUL終端を含まない。
成功時detail0は返却byte数（STATではinode）、detail1はfile type（STATではpacked metadata）。
CHMODのtargetはNUL終端文字列ではなく、permission/special mode `0000..7777`を
表す1 WordのLE整数。CHMOD成功時のdetail0/detail1はともに0。FOLLOW_FINALを
落としてsymlink自体を指定した場合はunsupportedを返し、link先を変更しない。

filesystem errorは専用status `0x91160001`とdetail0のerror codeで返す。
1=invalid、2=missing、3=not-directory、4=link-loop、5=too-long、6=exists、
7=I/O、8=no-space、9=read-only、10=unsupported。IPC/Capability errorとは別のerror domainであり、
IDLでは通常のAlpha statusと区別する必要がある。新file typeはVFS symlink=4、POSIX symlink=6。

### 16.3 Creation mode・stat mode・advisory lock

作成時modeは既存requestの未使用引数を拡張する。POSIX OPENとVFS
OPEN_COMPOUNDはarg3、POSIX MKDIR／CREATE_SOCKET_NODEはarg2、VFS
MKDIR／CREATEはarg3を使用する。0は従来のdefault、`(1 << 16) | permissions`
は明示指定（permissionは下位12bit）で、mode 0000とも区別する。
Linux umaskはAlterのFS contextに保持し、マスク適用後のmodeを転送する。
Kernelのmapping属性とは別のfilesystem metadataである。

POSIX／VFS STATのarg2、FSTATのarg1に`INCLUDE_MODE=1`を指定すると、
detail0は`(mode << 32) | inode`になる。inodeはext2の32bit番号、modeは
file typeを含む16bitのディスク値。detail1の既存packed metadataは変更しない。
flags=0の従来clientはinodeのみを受け取る。

FSTATのarg1に`INCLUDE_VERSION=2`を指定すると、detail0は
`(content_version << 32) | inode`になる。`INCLUDE_MODE`とは排他的で、
detail1のsize/type配置は変わらない。POSIXは通常のFD所有権確認後、VFSの
owned handleへ転送する。pathやmtimeだけをキーにした内容キャッシュの代用ではなく、
そのopen handleが現在指すinodeの内容を識別するためのopt-in拡張である。

versionはVFS instance内で再利用しない32bit token。内容の同サイズwrite、
truncate、inode metadata変更の前に失効させ、失敗後の部分的変更も含めて
古いtokenを再発行しない。hardlinkは同じinode/tokenを参照する。
VFSのtoken表は256件に制限され、追い出したinodeを再参照すると新しいtokenを
発行する。通常ファイル以外、VFS read-page leaseでpin中のinode、新規tokenを
発行できない場合はversion=0（キャッシュ不可）。tokenはVFSの再起動や別instanceを
跨いで比較してはいけない。version=0を「変更なし」と解釈してはいけない。

Alterはopen→FSTATで得たidentity/sizeに対してのみsnapshotを再利用し、cache missでは
positioned readの完了後に再度FSTATして、読み込み中にtoken/sizeが変わった場合は
失敗させる。FD offsetを動かさないため、delegated readから通常preadへのfallbackでも
途中のデータを飛ばさない。ゲストへのMAP_PRIVATEはsnapshotからのprivate copyであり、
書き込み可能なcache aliasを渡したり、MAP_SHAREDのcoherence契約を置き換えたりしない。

`POSIX_REQUEST_FCHMOD=0xa13e`、`VFS_REQUEST_FCHMOD=0x9117`はarg0にFD／
owned open handle、arg1にpermission/special mode `0000..7777`を受け取る。
shared bufferは使用しない。POSIXはsessionのFDからVFS handleへ変換する。
成功は通常のOKとdetail0/detail1=0、filesystem errorはPATHと同じ
`0x91160001`とdetail0のerror code。無効・別ownerのhandleは通常の
INVALID_DESCRIPTOR。pathnameへの再変換はせず、rename/unlink後も開いたinodeを
変更する。ext2のmodeと各open-handle cacheを更新し、既存writeback/fsyncで永続化する。

`POSIX_REQUEST_FLOCK=0xa13d`はarg0=FD、arg1=SHARED(1)／EXCLUSIVE(2)／
UNLOCK(8)、arg2=呼出元notification slot（0なら通知登録なし）。成功は通常の
OK、競合は専用status `0xa13d0001`。Server自体は待機しない。非zeroの通知slotを
登録したsessionには、unlock・lock変換・最終closeでidentifierのbit 21を通知する。
通知は競合判定より前に接続する。Alterは競合時に対象taskをparkし、通知後に
再試行するため、他のRPCを処理でき、check/park間の通知も取りこぼさない。
lockの所有者はPOSIX open-file description、対象はinodeであり、FD番号やPIDではない。

### ファイルシステム統計

`POSIX_REQUEST_FSTATFS=0xa141` はarg0=FD、`VFS_REQUEST_FSTATFS=0x911a`
はarg0=owned handle。成功時、各clientの登録済みshared bufferのoffset 0へ
`vfs::FilesystemStats`（80 bytes）を書き、通常OK、detail0/detail1=0を返す。
fieldはkind、block_size、blocks、free_blocks、available_blocks、files、
free_files、fsid、name_max、flagsの順で、各fieldはu64。POSIXはFD所有権を
検証してregular file／directoryのVFS handleへ変換し、VFS側bufferから
呼出元bufferへ転送する。buffer不足はINVALID_ARGUMENT、無効FD／別ownerの
handleはINVALID_DESCRIPTOR。ext2はwriteback cacheを通して現在のsuperblock
を読み、未flushの割当・解放も反映する。これはread-only RPCであり、継続的な
page leaseや新しいnotificationは作らない。

### 通常ファイルの読み取り共有page lease

`POSIX_REQUEST_MAP_READ_PAGE=0xa13f` / `UNMAP_READ_PAGE=0xa140`は
arg0=FD、arg1=4096-byte境界のfile offset。POSIXはowned regular-file FDを
VFS handleへ変換し、`VFS_REQUEST_MAP_READ_PAGE=0x9118` /
`UNMAP_READ_PAGE=0x9119`へ転送する。VFS側のarg0=owned handle、arg1=offset、
arg2=peer PIDである。POSIXがpeerへ設定するのは受信identifierであり、clientは
別PIDを指定できない。MAPの通常OK replyのdetail0はpeer側のpage VA、detail1=0。

VFSはinode/offsetごとに一つのNormal/WB RAM pageを保持し、Alphaの既存
MAPPING_SHAREでpeerへaliasを作る。これはbulk-transfer bufferでもprivateな
file snapshotでもない。通常WRITE/WRITE_DELEGATEDは該当pageの内容も更新する。
peerは自身のaliasと下流aliasを削除してからUNMAPを呼び、最後のleaseでVFS側
pageを解放する。leaseはowner/handle/peer/offsetで追跡し、callerはlease期間中
handleを保持する。Alterはhidden FDを保持し、fork/partial unmap/exec/exit時の
alias管理に既存memfd機構を使用する。guestのwrite permissionは付与しない。
peerであるmapper自身のaliasはAlphaの通常RAM共有でありR/Wである。read-onlyは
下流application mappingの契約であり、任意のclientへreadonly capabilityを配るAPIではない。

page lease中のfile size変更は専用status `VFS_RESPONSE_MAPPING_BUSY=0x91180001`
で拒否し、Alterは`EBUSY`へ変換する。VMA invalidationを実装したという意味ではない。
上限はVFS全体で1024 backing pages / 4096 leases。writable file共有、EOF以降の
初期mapping、lease中のresizeはこの契約に含まれない。
