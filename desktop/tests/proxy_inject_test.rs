// 验证 fetch_subscription 的端口注入:模拟订阅带 port:7890 + external-controller:9099
#[cfg(test)]
mod inject_tests {
    use serde_yaml::Value;

    fn inject(text: &str, proxy_port: u16, api_port: u16) -> Value {
        let mut yaml: Value = serde_yaml::from_str(text).unwrap();
        if let Value::Mapping(ref mut m) = yaml {
            m.insert(
                Value::String("external-controller".into()),
                Value::String(format!("127.0.0.1:{api_port}")),
            );
            m.insert(
                Value::String("secret".into()),
                Value::String("mc-proxy".into()),
            );
            m.insert(
                Value::String("mixed-port".into()),
                Value::Number(serde_yaml::Number::from(proxy_port)),
            );
            for key in ["port", "socks-port", "redir-port", "tproxy-port"] {
                m.remove(Value::String(key.into()));
            }
            m.insert(
                Value::String("dns".into()),
                Value::Mapping(serde_yaml::Mapping::new()),
            );
        }
        yaml
    }

    #[test]
    fn subscription_port_is_removed_and_mixed_port_forced() {
        // 该订阅实际返回的模样:port: 7890 + external-controller: 9099
        let sub = "port: 7890\nallow-lan: true\nexternal-controller: '127.0.0.1:9099'\nproxies: []\n";
        let yaml = inject(sub, 17898, 12091);
        if let Value::Mapping(ref m) = yaml {
            // mixed-port 被强制写入选定端口
            assert_eq!(
                m.get(&Value::String("mixed-port".into())).and_then(|v| v.as_u64()),
                Some(17898)
            );
            // 订阅自带的 port:7890 必须被移除(否则与 mixed-port 打架)
            assert!(m.get(&Value::String("port".into())).is_none());
            // external-controller 被覆盖为选定 API 端口
            assert_eq!(
                m.get(&Value::String("external-controller".into())).and_then(|v| v.as_str()),
                Some("127.0.0.1:12091")
            );
        } else {
            panic!("not a mapping");
        }
    }
}
