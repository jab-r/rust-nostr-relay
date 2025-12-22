//! Test REQ interception for KeyPackage queries

#[cfg(test)]
mod tests {
    use crate::mls_gateway::{MlsGateway, MlsGatewayConfig};
    use nostr_relay::{Extension, db::{Event, SortList}, ExtensionReqResult, PostProcessResult};
    use nostr_relay::message::Subscription;

    #[test]
    fn test_process_req_keypackage_query() {
        let config = MlsGatewayConfig::default();
        let gateway = MlsGateway::new(config);

        // Create a subscription that queries for KeyPackages (kind 443)
        let mut subscription = Subscription {
            id: "test_sub_1".to_string(),
            filters: vec![],
        };
        
        // Add filter for kind 443
        let mut filter = nostr_relay::db::Filter::default();
        filter.kinds = SortList::from(vec![443]);
        let author_bytes: [u8; 32] = hex::decode("1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef")
            .unwrap()
            .try_into()
            .unwrap();
        filter.authors = SortList::from(vec![author_bytes]);
        subscription.filters.push(filter);

        // Test process_req
        let result = gateway.process_req(1, &subscription);
        
        match result {
            ExtensionReqResult::Continue => {
                // Expected: we let the database query proceed
                println!("✓ process_req returned Continue for KeyPackage query");
            }
            _ => panic!("Expected Continue result for KeyPackage query"),
        }
    }

    #[test]
    fn test_process_req_non_keypackage_query() {
        let config = MlsGatewayConfig::default();
        let gateway = MlsGateway::new(config);

        // Create a subscription that queries for regular events (kind 1)
        let mut subscription = Subscription {
            id: "test_sub_2".to_string(),
            filters: vec![],
        };
        
        // Add filter for kind 1
        let mut filter = nostr_relay::db::Filter::default();
        filter.kinds = SortList::from(vec![1]);
        subscription.filters.push(filter);

        // Test process_req
        let result = gateway.process_req(1, &subscription);
        
        match result {
            ExtensionReqResult::Continue => {
                // Expected: non-KeyPackage queries should continue normally
                println!("✓ process_req returned Continue for non-KeyPackage query");
            }
            _ => panic!("Expected Continue result for non-KeyPackage query"),
        }
    }

    #[test]
    fn test_post_process_query_results_with_keypackages() {
        let config = MlsGatewayConfig::default();
        let gateway = MlsGateway::new(config);

        // Create a subscription
        let subscription = Subscription {
            id: "test_sub_3".to_string(),
            filters: vec![],
        };

        // Create mock KeyPackage events
        let mut events = Vec::new();
        
        // Create a KeyPackage event (kind 443)
        let keypackage_json = r#"{
            "id": "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            "pubkey": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            "created_at": 1700000000,
            "kind": 443,
            "tags": [
                ["p", "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"],
                ["mls_protocol_version", "1.0"],
                ["mls_ciphersuite", "0x0001"],
                ["exp", "1800000000"]
            ],
            "content": "0123456789abcdef",
            "sig": "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321"
        }"#;
        
        if let Ok(event) = serde_json::from_str::<Event>(keypackage_json) {
            events.push(event);
        }

        // Add a non-KeyPackage event
        let regular_event_json = r#"{
            "id": "2234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            "pubkey": "bbcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            "created_at": 1700000001,
            "kind": 1,
            "tags": [],
            "content": "Hello world",
            "sig": "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321"
        }"#;
        
        if let Ok(event) = serde_json::from_str::<Event>(regular_event_json) {
            events.push(event);
        }

        // Test post_process_query_results
        let result = gateway.post_process_query_results(1, &subscription, events.clone());
        
        // Verify results
        assert_eq!(result.events.len(), 2, "Should return all events");
        assert_eq!(result.consumed_events.len(), 0, "Should not mark any as consumed synchronously");
        
        println!("✓ post_process_query_results handled {} events correctly", result.events.len());
    }

    #[test]
    fn test_post_process_query_results_no_keypackages() {
        let config = MlsGatewayConfig::default();
        let gateway = MlsGateway::new(config);

        // Create a subscription
        let subscription = Subscription {
            id: "test_sub_4".to_string(),
            filters: vec![],
        };

        // Create only non-KeyPackage events
        let mut events = Vec::new();
        
        let regular_event_json = r#"{
            "id": "3234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            "pubkey": "cbcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            "created_at": 1700000002,
            "kind": 1,
            "tags": [],
            "content": "Just a regular event",
            "sig": "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321"
        }"#;
        
        if let Ok(event) = serde_json::from_str::<Event>(regular_event_json) {
            events.push(event);
        }

        // Test post_process_query_results
        let result = gateway.post_process_query_results(1, &subscription, events.clone());
        
        // Verify results
        assert_eq!(result.events.len(), 1, "Should return all events");
        assert_eq!(result.consumed_events.len(), 0, "Should not consume any events");
        
        println!("✓ post_process_query_results correctly ignored non-KeyPackage events");
    }
}