# 2026-10-01 P0 数据补全：磁盘物理盘/分区/UUID/标签、目录占用、网卡收发流量（三平台）

本批完成路线图 P0 的「磁盘」项：`DiskInfo` 扩展 label /
volume_uuid / partition_uuid / device_model / device_serial /
media_type 六个可选字段并接入三平台适配器；新增
`x disk usage <path> [--depth N]`（du 风格聚合排序，路径不存在退出码 3）
与 TUI 第五个 `disk` 页签（挂载表 + 启动目录的树形占用，`Enter`
展开/折叠，后台线程扫描不冻结事件循环）；网卡表新增收发流量列
（rx/tx 累计字节）。本机（Windows，zh-CN）实测通过；
Linux/macOS 走 `cargo check --tests --target`。

## Windows 卷路径句柄不吃磁盘 IOCTL，必须用 `\\.\C:` 盘符形态

现象：`IOCTL_DISK_GET_PARTITION_INFO_EX` 与
`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` 在
`GetVolumeNameForVolumeMountPointW` 返回的 `\\?\Volume{GUID}\`
句柄上双双失败 err=87（ERROR_INVALID_PARAMETER）。
处理：两个 IOCTL 改在 `\\.\C:`（sysinfo mount 的盘符形态）句柄上查询；
`\\?\Volume{...}\` 只用来取 volume_uuid。文件夹挂载的卷没有盘符形态，
诚实留 None。
证据：同一探针里 volume 句柄 ret=0/err=87，drive 句柄两码分别
ret=144、ret=32 全数返回。

## 卷根是目录对象：access-0 打开必须带 FILE_FLAG_BACKUP_SEMANTICS

现象：`CreateFileW(access=0)` 打开 `C:\` 或 `\\?\Volume{...}\` 直接
ERROR_PATH_NOT_FOUND(3)，看起来像路径不存在。
根因：卷根要按目录语义打开，目录打开需要 backup 语义位；同一路径串
换成设备形态 `\\.\C:` 又不需要该位。两个坑叠加极易误判成"路径错了"。
处理：open() 统一带 `FILE_ATTRIBUTE_NORMAL |
FILE_FLAG_BACKUP_SEMANTICS`，代码注释记录观测值。
证据：加位后 volume GUID 路径打开成功，GetVolumeInformationW 标签、
GetVolumeNameForVolumeMountPointW 全部工作。

## partmgr 在分区句柄上报的 PartitionStyle 与文档矛盾（本机 1，实际 GPT）

现象：本机三个卷（C:/D:/E:，CIM 均为 GPT: Basic Data）在
`\\.\X:` 句柄上 `PARTITION_INFORMATION_EX.PartitionStyle` 一律回 1
（文档枚举 MBR=1、GPT=2），若按文档值做门控，GPT 分区 UUID 永远拿不到。
排查：逐字节 hexdump 确认驱动的写入不是投影错位；再与
`MSFT_Partition`（root/Microsoft/Windows/Storage CIM provider，免管理员
可读）对照——三个卷的 `Gpt.PartitionId` 与 provider 的 `Guid` 逐字符
相等，`Gpt.PartitionType.data1` 也与 provider 的 `GptType` 一致，即
GPT 臂数据真实、只是 style 字段值异常。
处理：partition_uuid 的门控不看 style，改为白名单校验
`PartitionType` 首组 ∈ {BasicData, ESP, MSR, Recovery}。MBR 卷把
MBR 臂字节当 GPT 读时首组只可能是启动标志（≤0x0000FF80），永远命中
不了白名单，因此 MBR 依旧诚实返回 None，不产生编造的 UUID。
证据：`x disk list --json` 三卷 partition_uuid 与 MSFT provider 完全
一致；`live_volumes_report_label_uuid_and_disk_facts_where_permitted`
通过。

## 探针输出必须来自真正运行过的测试，`&&` 短路会制造"幻影输出"

现象：上一轮推理基于一份"探针显示 open 失败 err=3"的输出，后来发现
那次探针二进制根本没跑（cp 到不存在的目录 + `&&` 短路，错误被吞）。
根因：cargo test 管道里 grep 过滤后 exit code 丢失，且文件复制失败
短路了运行步骤，看到的"输出"是早前一次运行的残留。
处理：诊断只采信当次 `--nocapture` 完整回显；管道验证先看退出码再看
内容；诊断文件用 Write 工具重建，不用 sed（正则转义曾损坏过 Rust
源码里的反斜杠字面量）。

## STORAGE_QUERY_PROPERTY 免管理员可用（本机实测）

`\\.\PhysicalDriveN` 以 access 0 + backup 语义打开后，
`IOCTL_STORAGE_QUERY_PROPERTY`（StorageDeviceProperty 与
StorageDeviceSeekPenaltyProperty）都成功（ret=396），model/serial/
总线类型/寻道惩罚齐活，media_type 由 seek penalty 判定：本机
TOSHIBA HDWD110 → hdd，与常识一致。描述符字符串按
VendorIdOffset/ProductIdOffset 偏移寻址、NUL 截断、trim。

## GetIfEntry2 的正确姿势：只设 InterfaceIndex，LUID 槽保持 0

现象：按文档给 `MIB_IF_ROW2` 的 `SizeOfRow` 赋结构体大小（windows-sys
没投影该字段，它藏在与 `NET_LUID_LH` 共用的首 8 字节里，需借
`InterfaceLuid.Value` 写入）后，`GetIfEntry2` 反而开始失败。
根因：内核看到非零 LUID 就把它当查询键；写进去的"结构体大小"成了一个
不存在的接口 ID。
处理：LUID 槽保持 0、只设 `InterfaceIndex` 即可稳定取到
InOctets/OutOctets（本机实测）。windows-sys 0.61 的投影省略
`SizeOfRow` 恰恰让这条路径自然成立。
证据：`interfaces_carry_traffic_counters` 活测通过；
`x net interfaces` WLAN 行显示 763.4 MB / 983.4 MB。

## macOS 流量计数不硬编 if_data 偏移，走 `netstat -ib` 表头定列

`if_data` 结构体字段偏移随 macOS 版本变化，硬编是定时炸弹；改读
`netstat -ib`，按表头 token 的字节位置定列（BSD netstat 列内右对齐，
表头 token 起始位置起取第一个空白分隔 token 必属该列）。属 level-3
但非文本 locale 风险：token 是固定英文。取 `<Link#N>` 行（接口级计数
行），每接口首个生效。

## Linux 流量走 sysfs，堆叠设备解 slaves 链

`/sys/class/net/<if>/statistics/{rx,tx}_bytes` 直接读。磁盘侧
dm-*/md 堆叠设备经 `slaves` 递归展开（深度上限 3）找整盘，
`strip_partition_suffix` 按 nvme/mmcblk 的 `p<digits>` 与
sd/vd 的尾数形态剥离分区后缀（早先版本把"尾部非数字段"的长度
当成了"尾部数字段"的长度，方向反了，重写为
`chars().rev().take_while(is_ascii_digit)`）。

## du 语义：全树扫描、深度只过滤显示、不可读计数不致命

`x_core::walk_directory` 扫完整棵树，`--depth N` 只过滤上报行，
聚合值始终含全部后代（与 du 一致）；symlink_metadata 不跟随符号
链接；读不了的目录计入 `unreadable` 列而非报错中断；自底向上用
`split_at_mut(idx)` 反向归并避免 E0502 双重借用。路径不存在由 CLI
层映射为 Error::not_found → 退出码 3（实测 `exit=3`）。

## 遗留与边界

- Windows 卷 GUID 与分区 GUID 在本机逐字符相等，与 MSFT provider
  双重印证后确认是该机（存储栈）实况，非代码错误；常规机器上二者
  通常不同，两字段各自如实上报。
- 网络映射盘（\\server\share）无 `\\.\X:` 之外的磁盘 IOCTL 语义，
  扩展字段留 None、media_type 由 UNC 名称判定为 network。
- TUI disk 页后台扫描用一次性线程 + Arc<Mutex<Option<..>>> 信箱，
  on_tick 收结果；单次扫描制，与 ncdu 相同，无重扫键。
