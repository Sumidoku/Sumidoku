-- 注册时落库 SourceFeatures 序列化原文，DTO 据此暴露 hasHome 等能力位；
-- NULL = 旧行未 stamping，消费端按未知处理（不因此隐藏入口）
ALTER TABLE source ADD COLUMN features TEXT;
