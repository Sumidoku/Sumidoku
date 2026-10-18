use aidoku::{HomeComponent, HomeLayout, HomePartialResult};

/// get_home 流式调用的部分结果事件：Component 的下标已按上游 title 匹配语义
/// 解析好，消费者直接按数组下标替换/追加，不需要自己维护累积状态
#[derive(Debug, Clone, PartialEq)]
pub enum HomePartial {
	Layout(HomeLayout),
	Component { index: usize, component: HomeComponent },
}

/// 对齐 AidokuRunner getHome 的累积语义：Component 按 title 匹配替换（无匹配追加
/// 尾部）；源返回空终态布局时，部分结果的累积结果替代终态
#[derive(Debug, Default, Clone)]
pub struct HomeAccumulator {
	current: Vec<HomeComponent>,
	touched: bool,
}

impl HomeAccumulator {
	pub fn apply(&mut self, partial: HomePartialResult) -> HomePartial {
		self.touched = true;
		match partial {
			HomePartialResult::Layout(layout) => {
				self.current = layout.components.clone();
				HomePartial::Layout(layout)
			}
			HomePartialResult::Component(component) => {
				let index = self
					.current
					.iter()
					.position(|c| c.title == component.title)
					.unwrap_or(self.current.len());
				if index == self.current.len() {
					self.current.push(component.clone());
				} else {
					self.current[index] = component.clone();
				}
				HomePartial::Component { index, component }
			}
		}
	}

	pub fn finish(self, layout: HomeLayout) -> HomeLayout {
		if layout.components.is_empty() && self.touched {
			HomeLayout { components: self.current }
		} else {
			layout
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn titled(title: &str) -> HomeComponent {
		HomeComponent {
			title: Some(title.into()),
			..HomeComponent::default()
		}
	}

	#[test]
	fn component_upserts_by_title_then_appends() {
		let mut acc = HomeAccumulator::default();
		let HomePartial::Layout(_) = acc.apply(HomePartialResult::Layout(HomeLayout {
			components: vec![titled("a"), titled("b")],
		})) else {
			panic!("expected layout");
		};
		let HomePartial::Component { index, .. } = acc.apply(HomePartialResult::Component(titled("a"))) else {
			panic!("expected component");
		};
		assert_eq!(index, 0);
		let HomePartial::Component { index, .. } = acc.apply(HomePartialResult::Component(titled("c"))) else {
			panic!("expected component");
		};
		assert_eq!(index, 2);
	}

	#[test]
	fn empty_final_falls_back_to_accumulated() {
		let mut acc = HomeAccumulator::default();
		acc.apply(HomePartialResult::Component(titled("a")));
		let layout = acc.finish(HomeLayout::default());
		assert_eq!(layout.components.len(), 1);
	}

	#[test]
	fn non_empty_final_wins() {
		let mut acc = HomeAccumulator::default();
		acc.apply(HomePartialResult::Component(titled("a")));
		let layout = acc.finish(HomeLayout {
			components: vec![titled("z")],
		});
		assert_eq!(layout.components.len(), 1);
		assert_eq!(layout.components[0].title.as_deref(), Some("z"));
	}

	#[test]
	fn no_partials_keeps_empty_final() {
		let acc = HomeAccumulator::default();
		assert!(acc.finish(HomeLayout::default()).components.is_empty());
	}
}
