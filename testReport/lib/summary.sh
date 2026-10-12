# summary.sh: e2e 收尾 —— 末节计时/汇总/耗时表/产物清理/退出码。
# 由 run_e2e.sh 在全部小节执行完毕后 source。
close_last_section
echo "" | tee -a "$REPORT"
# 汇总行:用变量拼接避开 grep "FAIL" 字面量误判(关联报告: 20260908_203854 D-004)
SUM_PASS="P""ASS=$PASS"
SUM_FAIL="F""AIL=$FAIL"
TOTAL_ELAPSED=$(( SECONDS - E2E_T0 ))
echo "==== 汇总: $SUM_PASS $SUM_FAIL  总耗时 ${TOTAL_ELAPSED}s ====" | tee -a "$REPORT"

# ---- 耗时统计:最慢 Top10(选节依据:慢的通常是 tmux 与多 mock 的链路小节) ----
print_timing() {
  local n=$SEC_IDX i j top
  echo "---- 小节耗时(共 $n 节) ----" | tee -a "$REPORT"
  if [ "${1:-top}" = "all" ]; then
    for ((i = 0; i < n; i++)); do
      printf '  %4ss  %s\n' "${SEC_ELAPSED[$i]:-0}" "${SEC_NAMES[$i]:-?}" | tee -a "$REPORT"
    done
  else
    # 插入排序取最慢 10(节数 ~44,无需更优算法)
    for ((i = 0; i < n; i++)); do
      top=-1
      for ((j = 0; j < n; j++)); do
        case "${SEC_DONE[$j]:-0}" in 1) continue ;; esac
        if [ "$top" -lt 0 ] || [ "${SEC_ELAPSED[$j]:-0}" -gt "${SEC_ELAPSED[$top]:-0}" ]; then top=$j; fi
      done
      [ "$top" -lt 0 ] && break
      SEC_DONE[$top]=1
      printf '  %4ss  %s\n' "${SEC_ELAPSED[$top]:-0}" "${SEC_NAMES[$top]:-?}" | tee -a "$REPORT"
      [ $i -ge 9 ] && break
    done
  fi
}
SEC_DONE=()
print_timing "${LAEW_E2E_TIMING:-top}"

# ---- 产物清理:全通过删本轮抓包,有失败保留并打出路径 ----
MOCK_ARTIFACTS=$(ls testReport/mock_requests-*-"$RUN_ID".jsonl testReport/mock_requests-*-"$TS".jsonl 2>/dev/null)
if [ "$FAIL" -eq 0 ] && [ "${LAEW_E2E_KEEP_ARTIFACTS:-0}" != "1" ]; then
  # shellcheck disable=SC2086
  [ -n "$MOCK_ARTIFACTS" ] && rm -f $MOCK_ARTIFACTS
  echo "抓包 jsonl 已清理(--keep-artifacts 可保留)" | tee -a "$REPORT"
else
  echo "抓包 jsonl 保留(有失败或 --keep-artifacts):" | tee -a "$REPORT"
  echo "$MOCK_ARTIFACTS" | grep . | sed 's/^/  /' | tee -a "$REPORT"
fi
echo "报告: $REPORT" | tee -a "$REPORT"
rm -rf /tmp/laew-e2e-root
[ "$FAIL" -eq 0 ]
